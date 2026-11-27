//! The open page, as the Nexus app's (`ui/src/components/nexus-ui/pages/page-panel.tsx`): an artifact or a Nexus
//! Page under a header (`page_header.rs`) with its title menu, Share, full view, and close. It shows in the Workbench's Artifacts
//! tab (`side_pane.rs`) on every screen, Chat included. A page is a [`WebView`], so it draws inside this
//! window like any other element; a shared chat is its Markdown, drawn natively.
//!
//! Full view is the Workbench maximized on Artifacts. The header's button and [`TogglePageFullView`] turn
//! it on and off, Esc leaves it, and so do a design canvas's own Full view button and Escape (`page_canvas.rs`). A
//! design or a deck the person opened starts in full view, once per page in a run of the app.
//!
//! A board's ⋯, as in Nexus UI: the canvas's Regenerate, Make 3 variants or Keep this variant shows
//! a bar under the header with the question, its action and Cancel. The page's own scripts can post the same message,
//! so nothing runs before that click. Then Gateway opens a chat of the person's on the page
//! (`POST /v1/pages/{id}/boards/ask`), and the app goes there and sends its instruction, whose turn writes the next
//! version.

use std::time::{Duration, Instant};

use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::page_canvas::{self, Ask, Kind, Message};
use super::page_loading::{self, PageLoading};
use super::side_pane::Tool;
use super::{Place, Workspace, transcript};
use crate::gateway::{ChatText, PageFrame, Verb};
use crate::library::Row;
use crate::model::SessionSummary;
use crate::pages::{fetch, ui};
use crate::palette::Palette;
use crate::web::{WebEvent, WebView};

actions!(workspace, [TogglePageFullView]);

pub(super) struct PagePanel {
    title: SharedString,
    /// Nexus UI's address for it, for Open in browser; none for a page opened by address.
    link: Option<String>,
    content: Content,
    /// The chat answer's page and version it shows, so that answer's card reads "Opened".
    pub(super) block: Option<(String, u32)>,
    /// Gateway's `page@version` for what it shows, when it named one: the key a kept page shows again by.
    version_key: Option<String>,
    /// The page's id, when Gateway named it: what a board ask goes to.
    page_id: Option<String>,
    /// The Library row the person opened it from, if they did.
    opened_row: Option<String>,
    /// Its item in the Library, for the header's title menu and Share: found again whenever the lists arrive.
    item: Option<Row>,
    /// Whether someone else can open the item (`Lists::shared`), for Share's label: found with it.
    shared: bool,
    /// A board ask from the canvas, waiting for the person's click in the bar.
    board_pending: Option<(Ask, String)>,
    /// What runs while Gateway opens the chat, or why it could not.
    board_busy: Option<String>,
    board_error: Option<String>,
    /// Gateway named the page: its title holds, and the document's own `<title>` does not replace it.
    named: bool,
    /// What shows while the page opens, until it faded out over the page; and whether the page shows.
    loading: Option<Entity<PageLoading>>,
    page_shown: bool,
    /// Where a page loads from, for loading it again; none for a shared chat.
    source: Option<PageSource>,
    /// The page failed once and loaded again on its own: the next failure says so instead.
    retried: bool,
    _title: Option<Subscription>,
}

/// What a try of a page loads, and what its failure report names.
#[derive(Clone)]
struct PageSource {
    /// Its signed address, which expires ([`expired`]).
    url: String,
    /// Gateway's path for a fresh address ([`PageFrame::mint`]).
    mint: Option<String>,
    host: Option<String>,
    report_id: Option<String>,
    version_key: Option<String>,
    /// The page by its Nexus UI address, and whether the app opened it on its own: the open-in-full-view-once rule.
    key: String,
    auto: bool,
}

/// How long a page may take to load before it counts as failed, unless it drew.
const LOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// What a failed try of a page leads to.
#[derive(Debug, PartialEq)]
enum AfterFailure {
    /// A notice of ours (403, 404, 502) explains itself, and would only show again: it stays.
    Keep,
    /// The first failure the engine reports (no answer, a reset, a timeout): the page loads again, on its own.
    Retry,
    /// The second: the failure card ([`page_loading::failed`]) replaces the engine's error page.
    Explain,
}

/// What follows a failed try: `status` is the host's answer, if it sent one.
fn after_failure(status: Option<u16>, retried: bool) -> AfterFailure {
    match status {
        Some(_) => AfterFailure::Keep,
        None if retried => AfterFailure::Explain,
        None => AfterFailure::Retry,
    }
}

/// Whether the signed page address `url` is no use any more at `now` (Unix seconds): its `exp` (`pages-host`), less
/// the time a load takes, has passed. An address without one never expires.
fn expired(url: &str, now: u64) -> bool {
    const MARGIN: u64 = 30;
    url::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.query_pairs()
                .find(|(name, _)| name == "exp")
                .and_then(|(_, exp)| exp.parse::<u64>().ok())
        })
        .is_some_and(|exp| exp <= now + MARGIN)
}

/// The host a page is served from, for its failure report and card: only a plain host name.
fn page_host(url: &str) -> Option<String> {
    url::Url::parse(url).ok().and_then(|url| {
        let host = url.host_str()?.to_ascii_lowercase();
        (host.len() <= 253
            && host
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte)))
        .then_some(host)
    })
}

/// Whether the document's own `<title>` becomes the header's: only when Gateway named none, and never the blank
/// document the page opens on while its startup script goes in place.
fn takes_document_title(named: bool, title: &str) -> bool {
    !named && title != "about:blank" && !title.trim().is_empty()
}

enum Content {
    Web(Entity<WebView>),
    Text(Entity<TextViewState>, ScrollHandle),
    /// A page loading again waits for Gateway to sign a fresh address: only the outline shows.
    Pending,
    /// A page that did not load, twice: the failure card.
    Failed,
}

impl PagePanel {
    /// Whether this panel shows `view`.
    fn shows(&self, view: &Entity<WebView>) -> bool {
        self.shows_id(view.entity_id())
    }

    fn shows_id(&self, view: EntityId) -> bool {
        matches!(&self.content, Content::Web(shown) if shown.entity_id() == view)
    }

    /// The failure card replaces the page, and its web view closes.
    fn fail(&mut self) {
        self.content = Content::Failed;
        self.loading = None;
        self.page_shown = false;
        self._title = None;
    }
}

/// How many pages the Artifacts tab keeps loaded besides the one it shows, and for how long. Going back to it shows it
/// at once: no Gateway round trip, no load, no picture. One covers going back and forth between two artifacts or two
/// versions. Each kept page is a page in the web engine with its memory (about 200 MiB of resident memory for a page
/// of images, measured), and the engine does not exit for idle while one is kept: so one, for as long as the engine
/// waits for idle (`web/client.rs`).
const KEPT_PAGES: usize = 1;
const KEEP_PAGE_FOR: Duration = Duration::from_secs(180);

/// A page the Artifacts tab showed, by Gateway's `page@version`. A version never changes, so it shows again as it was.
/// Another sign-in or organization drops them all ([`Workspace::drop_kept_pages`]).
pub(super) struct KeptPage {
    key: String,
    panel: PagePanel,
    at: Instant,
}

/// The loading outline, under the page while it opens and over it once it shows (to fade out over it).
fn loading_layers<T: Clone>(loading: Option<&T>, shown: bool) -> (Option<T>, Option<T>) {
    match loading {
        Some(loading) if shown => (None, Some(loading.clone())),
        Some(loading) => (Some(loading.clone()), None),
        None => (None, None),
    }
}

/// Record completed loads before filtering events from an inactive page.
fn track_page_event(event: &WebEvent, address: Option<&str>, shown: bool, loaded: &AtomicBool) -> bool {
    if let WebEvent::Loading(state) = event
        && !state.loading
        && address.is_some_and(|address| address != "about:blank")
    {
        loaded.store(true, Ordering::Relaxed);
    }
    shown
}

impl Workspace {
    /// Keep the shown page loaded, hidden, if it is a version of a page; the oldest past [`KEPT_PAGES`] closes.
    fn keep_shown_page(&mut self, cx: &mut Context<Self>) {
        let Some(mut panel) = self.page.take() else {
            return;
        };
        let Some(key) = panel.version_key.clone() else {
            return;
        };
        // A hidden page may finish opening while its events are filtered. Keep only pages already shown.
        if !panel.page_shown || !matches!(panel.content, Content::Web(_)) {
            return;
        }
        panel.loading = None;
        self.kept_pages.retain(|kept| kept.key != key);
        self.kept_pages.push(KeptPage {
            key,
            panel,
            at: Instant::now(),
        });
        if self.kept_pages.len() > KEPT_PAGES {
            self.kept_pages.remove(0);
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(KEEP_PAGE_FOR).await;
            let _ = this.update(cx, |this, _| this.drop_stale_pages());
        })
        .detach();
    }

    fn drop_stale_pages(&mut self) {
        self.kept_pages.retain(|kept| kept.at.elapsed() < KEEP_PAGE_FOR);
    }

    /// Close every kept page: they are another sign-in's.
    pub(super) fn drop_kept_pages(&mut self) {
        self.kept_pages.clear();
    }

    /// Show the kept page `key` (`page@version`) again, in place of the shown one. False when none is kept.
    pub(super) fn show_kept_page(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        self.drop_stale_pages();
        let Some(at) = self.kept_pages.iter().position(|kept| kept.key == key) else {
            return false;
        };
        let KeptPage { mut panel, .. } = self.kept_pages.remove(at);
        self.keep_shown_page(cx);
        if let Some(block) = self.page_opening.take() {
            panel.block = Some(block);
        }
        self.page_opening_auto = false;
        self.library_opening = None;
        self.page = Some(panel);
        self.find_page_item();
        self.side.page_replaced();
        crate::measure::page_mark("view");
        crate::measure::page_mark("usable");
        cx.notify();
        true
    }
    /// Show `frame` in the Workbench's Artifacts tab, replacing what was there.
    pub(super) fn open_page(&mut self, frame: PageFrame, window: &mut Window, cx: &mut Context<Self>) {
        if !self.accepts_automatic_page(&frame.path, window.is_window_active()) {
            self.page_opening_auto = false;
            self.page_opening = None;
            cx.notify();
            return;
        }
        // A version shown lately is still loaded: it shows again as it was, and Gateway's new address goes unused.
        if let Some(key) = &frame.version_key
            && self.show_kept_page(key, cx)
        {
            return;
        }
        // `page@version` (`gateway::version_key`): page ids have no `@`.
        let page_id = frame
            .version_key
            .as_deref()
            .and_then(|key| key.rsplit_once('@'))
            .map(|(page, _)| page.to_string());
        let source = PageSource {
            host: page_host(&frame.url),
            url: frame.url.clone(),
            mint: frame.mint.clone(),
            report_id: page_id.clone(),
            version_key: frame.version_key.clone(),
            // The page, by its Nexus UI address (one for all its versions), for the open-in-full-view-once rule; and
            // whether the app opened it on its own, beside a running chat.
            key: if frame.path.is_empty() {
                frame.url.clone()
            } else {
                frame.path.clone()
            },
            auto: std::mem::take(&mut self.page_opening_auto),
        };
        let (view, loading, title) = self.page_view(&source, window, cx);
        let link = (!frame.path.is_empty()).then(|| format!("{}{}", self.gateway.site_url(), frame.path));
        self.keep_shown_page(cx);
        let named = frame.title.as_deref().is_some_and(|title| !title.trim().is_empty());
        self.page = Some(PagePanel {
            title: frame.title.filter(|_| named).unwrap_or_else(|| "Page".into()).into(),
            link,
            content: Content::Web(view),
            block: self.page_opening.take(),
            version_key: frame.version_key,
            page_id,
            opened_row: self.library_opening.take(),
            item: None,
            shared: false,
            board_pending: None,
            board_busy: None,
            board_error: None,
            named,
            loading: Some(loading),
            page_shown: false,
            source: Some(source),
            retried: false,
            _title: Some(title),
        });
        self.find_page_item();
        self.side.page_replaced();
        crate::measure::page_mark("view");
        cx.notify();
    }

    /// One try of `page`: a new web view on its address, behind the loading outline, whose failure is reported and
    /// handled ([`Workspace::page_failed`]).
    fn page_view(
        &mut self,
        page: &PageSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<WebView>, Entity<PageLoading>, Subscription) {
        let view = cx.new(|cx| {
            let mut view = WebView::with_startup(&page.url, page_canvas::STARTUP, window, cx);
            // No engine white and no blank startup document: the loading outline shows until the page drew.
            view.veil();
            // The version as it last looked shows while it opens; a new version has no picture yet.
            if let Some(key) = &page.version_key {
                view.keep_snapshot(key, cx);
            }
            view
        });
        let poster = view.read(cx).has_placeholder();
        let loading = cx.new(|_| PageLoading::new(poster));
        let key = page.key.clone();
        let auto = page.auto;
        let loaded = Arc::new(AtomicBool::new(false));
        let reported = Arc::new(AtomicBool::new(false));
        let host = page.host.clone();
        let report_id = page.report_id.clone();
        let loaded_on_event = loaded.clone();
        let reported_on_event = reported.clone();
        let host_on_event = host.clone();
        let id_on_event = report_id.clone();
        let view_id = view.entity_id();
        let title = cx.subscribe_in(
            &view,
            window,
            move |this, source, event: &WebEvent, window, cx| match event {
                _ if !track_page_event(
                    event,
                    source.read(cx).address().as_deref(),
                    this.page.as_ref().is_some_and(|panel| panel.shows(source)),
                    &loaded_on_event,
                ) => {}
                WebEvent::TitleChanged(title) => {
                    if let Some(panel) = &mut this.page
                        && takes_document_title(panel.named, title)
                        && panel.title.as_ref() != title.as_str()
                    {
                        panel.title = title.clone().into();
                        cx.notify();
                    }
                }
                // The page drew: the outline fades out over it, then goes.
                WebEvent::Shown => {
                    if let Some(panel) = &mut this.page {
                        panel.page_shown = true;
                        if let Some(loading) = panel.loading.clone() {
                            loading.update(cx, |loading, cx| loading.leave(cx));
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(crate::motion::UI).await;
                                // Fails only when the window closed meanwhile: then there is nothing to clear.
                                this.update(cx, |this, cx| {
                                    if let Some(panel) = &mut this.page
                                        && panel.loading.as_ref() == Some(&loading)
                                    {
                                        panel.loading = None;
                                        cx.notify();
                                    }
                                })
                                .ok();
                            })
                            .detach();
                        }
                        cx.notify();
                    }
                }
                WebEvent::Cdp(message) => match page_canvas::parse(message) {
                    Some(Message::FullView) => {
                        this.side.toggle_page_full();
                        cx.notify();
                    }
                    Some(Message::Escape) if this.side.leave_page_full() => cx.notify(),
                    Some(Message::Ready(kind)) => {
                        // A design zooms with a touchpad pinch; a deck does not take one.
                        source.update(cx, |view, _| view.set_pinch_zoom(kind == Kind::Design));
                        // As Nexus UI: the page's once is used up on its first ready, even if it is in full view already.
                        if page_canvas::first_full_view(&key, auto) && !this.side.page_full() {
                            this.side.toggle_page_full();
                            cx.notify();
                        }
                    }
                    // Only staged: the ask runs on a click in the bar, never on the page's word alone. The question on
                    // screen stays until the person answers it.
                    Some(Message::Board { ask, board }) => {
                        if let Some(panel) = &mut this.page {
                            panel.board_error = None;
                            panel.board_pending.get_or_insert((ask, board));
                            cx.notify();
                        }
                    }
                    Some(Message::Escape) | None => {}
                },
                WebEvent::LoadFailed(status) if !reported_on_event.swap(true, Ordering::Relaxed) => {
                    this.report_frame_failure(
                        if status.is_some() { "http" } else { "error" },
                        *status,
                        host_on_event.clone(),
                        id_on_event.clone(),
                        cx,
                    );
                    // After this event: the next try replaces the view that sent it.
                    let status = *status;
                    cx.defer_in(window, move |this, window, cx| {
                        this.page_failed(view_id, status, window, cx)
                    });
                }
                // The address and popups are for a session's browser, which the panel is not.
                _ => {}
            },
        );
        // Slow: a page that drew but never finished loading shows; one that has not drawn says it is still loading.
        let slow_view = view.downgrade();
        let slow_loading = loading.downgrade();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(page_loading::SLOW_AFTER).await;
            // Fails only when the page closed meanwhile: then it is not slow.
            slow_view.update(cx, |view, cx| view.reveal_if_drawn(cx)).ok();
            slow_loading.update(cx, |loading, cx| loading.set_slow(cx)).ok();
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(LOAD_TIMEOUT).await;
            // Fails only when the window closed meanwhile.
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(panel) = this.page.as_ref().filter(|panel| panel.shows_id(view_id)) else {
                    return;
                };
                if !loaded.load(Ordering::Relaxed) && !reported.swap(true, Ordering::Relaxed) {
                    // A page that drew stays, however long it takes to finish; one that drew nothing failed.
                    let drew = panel.page_shown;
                    this.report_frame_failure("timeout", None, host, report_id, cx);
                    if !drew {
                        this.page_failed(view_id, None, window, cx);
                    }
                }
            });
        })
        .detach();
        (view, loading, title)
    }

    /// The open page's try in `view` failed (`status` when the host answered one): load it again, once, or say so.
    fn page_failed(&mut self, view: EntityId, status: Option<u16>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.page.as_mut().filter(|panel| panel.shows_id(view)) else {
            return;
        };
        match after_failure(status, panel.retried) {
            AfterFailure::Keep => {}
            AfterFailure::Retry => {
                panel.retried = true;
                self.load_page_again(window, cx);
            }
            AfterFailure::Explain => {
                panel.fail();
                cx.notify();
            }
        }
    }

    /// Try again, on the failure card.
    fn retry_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            &self.page,
            Some(PagePanel {
                content: Content::Failed,
                ..
            })
        ) {
            self.load_page_again(window, cx);
        }
    }

    /// Load the open page again in a new web view; first a fresh signed address from Gateway when its own expired.
    fn load_page_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(panel) = &mut self.page else { return };
        let Some(source) = panel.source.clone() else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let Some(mint) = source.mint.clone().filter(|_| expired(&source.url, now)) else {
            return self.show_page_try(source, window, cx);
        };
        // Only the outline, while Gateway signs it again.
        panel.content = Content::Pending;
        panel.loading = Some(cx.new(|_| PageLoading::new(false)));
        panel.page_shown = false;
        panel._title = None;
        cx.notify();
        let request = account_gateway.request(Verb::Get, mint, None);
        cx.spawn_in(window, async move |this, cx| {
            let answer = request.await;
            // Fails only when the window closed meanwhile.
            let _ = this.update_in(cx, |this, window, cx| {
                // A page opened since is not this one.
                let Some(panel) = this.page.as_mut().filter(|panel| {
                    matches!(panel.content, Content::Pending)
                        && panel.source.as_ref().is_some_and(|now| now.url == source.url)
                }) else {
                    return;
                };
                match answer.ok().and_then(|answer| crate::model::text(&answer, "url")) {
                    Some(url) => {
                        let source = PageSource { url, ..source };
                        panel.source = Some(source.clone());
                        this.show_page_try(source, window, cx);
                    }
                    None => {
                        panel.fail();
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Show a new try of the open page, `source`, in place of what it shows.
    fn show_page_try(&mut self, source: PageSource, window: &mut Window, cx: &mut Context<Self>) {
        let (view, loading, title) = self.page_view(&source, window, cx);
        if let Some(panel) = &mut self.page {
            panel.content = Content::Web(view);
            panel.loading = Some(loading);
            panel.page_shown = false;
            panel._title = Some(title);
        }
        cx.notify();
    }

    fn report_frame_failure(
        &self,
        kind: &'static str,
        status: Option<u16>,
        host: Option<String>,
        artifact_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !nexus_desktop_core::usage::allowed() {
            return;
        }
        let Some(gateway) = self.account_gateway() else { return };
        let body = json!({"failures": [{
            "surface": "subdomain", "kind": kind, "app": "desktop-gpui", "appVersion": crate::channel::VERSION,
            "browser": "chrome", "host": host, "artifactId": artifact_id, "status": status
        }]});
        let request = gateway.request(Verb::Post, "/v1/diagnostics/frame-failures".into(), Some(body));
        cx.spawn(async move |_, _| {
            let _ = request.await;
        })
        .detach();
    }

    /// Show a shared chat's Markdown in the Workbench's Artifacts tab, replacing what was there.
    pub(super) fn open_chat_text(&mut self, chat: ChatText, cx: &mut Context<Self>) {
        let text = cx.new(|cx| TextViewState::markdown(crate::library::without_front_matter(&chat.markdown), cx));
        self.page = Some(PagePanel {
            title: chat.title.into(),
            link: Some(format!("{}{}", self.gateway.site_url(), chat.path)),
            content: Content::Text(text, ScrollHandle::new()),
            block: None,
            version_key: None,
            page_id: None,
            opened_row: self.library_opening.take(),
            item: None,
            shared: false,
            board_pending: None,
            board_busy: None,
            board_error: None,
            named: true,
            loading: None,
            page_shown: true,
            source: None,
            retried: false,
            _title: None,
        });
        self.find_page_item();
        self.side.page_replaced();
        cx.notify();
    }

    /// The open page's Library item, for the header's title menu and Share.
    pub(super) fn page_item(&self) -> Option<&Row> {
        self.page.as_ref()?.item.as_ref()
    }

    /// The open page's address in Nexus UI, for the title menu's Open in browser when the Library has no item for it.
    pub(super) fn page_link(&self) -> Option<&str> {
        self.page.as_ref()?.link.as_deref()
    }

    /// Whether someone else can open the open page: Share then reads Shared.
    pub(super) fn page_shared(&self) -> bool {
        self.page.as_ref().is_some_and(|panel| panel.shared)
    }

    /// Find the open page's item in the Library's lists. A page the lists do not have yet (one a chat just made, or
    /// none read since sign-in) reads them again, and the item shows when they arrive (`library_loaded`).
    fn find_page_item(&mut self) {
        if self.refresh_page_item() {
            return;
        }
        if self
            .page
            .as_ref()
            .is_some_and(|panel| panel.opened_row.is_some() || panel.page_id.is_some())
        {
            self.reload_library();
        }
    }

    /// Match the open page against the lists last read; true when it has an item.
    pub(super) fn refresh_page_item(&mut self) -> bool {
        let Some(panel) = &mut self.page else {
            return false;
        };
        let lists = match &self.library.lists {
            Some(Ok(lists)) => Some(lists),
            _ => None,
        };
        // A failed or pending read keeps the item already found.
        if let Some(lists) = lists {
            panel.item = lists
                .item_for(panel.opened_row.as_deref(), panel.page_id.as_deref())
                .cloned();
            panel.shared = panel.item.as_ref().is_some_and(|item| lists.shared(item));
        }
        panel.item.is_some()
    }

    /// Close the page and its Artifacts tab: the Workbench shows another tool, or closes when none is open.
    pub(super) fn close_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tool(Tool::Artifacts, window, cx);
    }

    /// Whether the page has the keys: its Esc is then the page's, and a canvas says when it has nothing to clear.
    pub(super) fn page_focused(&self, window: &Window, cx: &App) -> bool {
        matches!(&self.page, Some(PagePanel { content: Content::Web(view), .. })
            if view.focus_handle(cx).contains_focused(window, cx))
    }

    /// The page in full view, or the Workbench back beside the session.
    pub(super) fn toggle_page_full_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page.is_none() {
            return;
        }
        if self.side.toggle_page_full() {
            // The composer is hidden: the keys go to the Workbench, so Esc leaves full view.
            window.focus(&self.focus, cx);
        } else if !self.side_terminal_focused(window, cx) {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    pub(super) fn on_toggle_page_full_view(
        &mut self,
        _: &TogglePageFullView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_page_full_view(window, cx);
    }

    /// The person confirmed the board ask in the bar: Gateway opens a chat of theirs on the page (Nexus UI's
    /// `askNexusOnBoard`), and the app goes there. Gateway refuses a page that is not theirs.
    fn ask_board(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let Some(panel) = &mut self.page else { return };
        if panel.board_busy.is_some() {
            return;
        }
        let Some((ask, board)) = panel.board_pending.take() else {
            return;
        };
        let Some(page_id) = panel.page_id.clone() else {
            // Only a page opened by address (`NEXUS_WEB_URL`) has none.
            panel.board_error = Some("Gateway did not name this page, so Nexus can't change it from here.".into());
            return cx.notify();
        };
        panel.board_busy = Some(ask.busy(&board));
        let path = format!("/v1/pages/{}/boards/ask", nexus_client::url::enc(&page_id));
        let body = json!({ "ask": ask.wire(), "board": board });
        let request = account_gateway.request(Verb::Post, path, Some(body));
        fetch(cx, request, move |this, answer, cx| {
            let asked = answer.and_then(|asked| {
                let session = SessionSummary::from_json(&asked["session"]);
                let instruction = crate::model::text(&asked, "instruction");
                session
                    .zip(instruction)
                    .ok_or_else(|| "Gateway sent no chat for it.".to_string())
            });
            // A page opened since keeps its own bar.
            if let Some(panel) = this
                .page
                .as_mut()
                .filter(|panel| panel.page_id.as_deref() == Some(page_id.as_str()))
            {
                panel.board_busy = None;
                panel.board_error = asked.as_ref().err().cloned();
            }
            if let Ok((session, instruction)) = asked {
                this.open_board_chat(session, instruction, cx);
            }
        });
        cx.notify();
    }

    /// Go to the chat Gateway opened and send its instruction as the first message, as Nexus UI's hand-off does. The
    /// chat is new, so there is nothing to read first. The page stays beside it, out of full view, so both show.
    fn open_board_chat(&mut self, session: SessionSummary, instruction: String, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        self.side.leave_page_full();
        self.visit(Place::Chat(Some(session.clone())));
        self.reset_chat();
        self.moving = None;
        self.current = Some(session.id);
        self.title = session.title.into();
        self.ask(instruction, Default::default(), cx);
        account_gateway.list_sessions();
    }

    fn cancel_board_ask(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = &mut self.page {
            panel.board_pending = None;
            panel.board_error = None;
            cx.notify();
        }
    }

    /// The board ask's bar under the header: running, the question with its action and Cancel, or what went wrong.
    fn render_board_bar(&self, panel: &PagePanel, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let bar = h_flex()
            .flex_none()
            .px(px(12.))
            .py(px(8.))
            .gap(px(8.))
            .border_b_1()
            .border_color(palette.border)
            .text_size(px(13.))
            .text_color(palette.foreground);
        let line = |text: String| div().flex_1().min_w(px(0.)).child(text);
        if let Some(busy) = &panel.board_busy {
            return Some(
                bar.text_color(palette.muted)
                    .child(line(busy.clone()))
                    .into_any_element(),
            );
        }
        if let Some((ask, board)) = &panel.board_pending {
            let (question, action) = ask.confirm(board);
            return Some(
                bar.child(line(question))
                    .child(
                        ui::primary_button("page-board-ask", action, palette)
                            .on_click(cx.listener(|this, _, _, cx| this.ask_board(cx))),
                    )
                    .child(
                        ui::ghost_button("page-board-cancel", "Cancel", palette)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_board_ask(cx))),
                    )
                    .into_any_element(),
            );
        }
        let error = panel.board_error.clone()?;
        Some(
            bar.child(line(error).text_color(palette.destructive))
                .child(
                    ui::ghost_button("page-board-dismiss", "Dismiss", palette)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_board_ask(cx))),
                )
                .into_any_element(),
        )
    }

    /// The open page under its header (`page_header.rs`), `header` tall; `narrow` when the Workbench is too narrow for
    /// Share's label. On Artifacts the header's Full view replaces the Workbench's maximize.
    pub(super) fn render_page(
        &self,
        palette: Palette,
        header: f32,
        narrow: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = self.page.as_ref()?;
        let header = self.render_page_header(panel.title.clone(), header, narrow, palette, cx);
        let board_bar = self.render_board_bar(panel, palette, cx);
        Some(
            v_flex()
                .size_full()
                .child(header)
                .children(board_bar)
                .child(match &panel.content {
                    // The outline lies under the see-through page while it opens, and over it while it fades out.
                    Content::Web(view) => {
                        let (under, over) = loading_layers(panel.loading.as_ref(), panel.page_shown);
                        div()
                            .relative()
                            .flex_1()
                            .min_h(px(0.))
                            .bg(palette.background)
                            .children(under)
                            .child(view.clone())
                            .children(over)
                            .into_any_element()
                    }
                    Content::Pending => div()
                        .relative()
                        .flex_1()
                        .min_h(px(0.))
                        .bg(palette.background)
                        .children(panel.loading.clone())
                        .into_any_element(),
                    Content::Failed => {
                        let host = panel.source.as_ref().and_then(|source| source.host.as_deref());
                        let retry = ui::primary_button("page-retry", "Try again", palette)
                            .debug_selector(|| "page-retry".into())
                            .on_click(cx.listener(|this, _, window, cx| this.retry_page(window, cx)));
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .child(page_loading::failed(host, palette, retry))
                            .into_any_element()
                    }
                    Content::Text(text, scroll) => div()
                        .id("page-text")
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scroll()
                        .track_scroll(scroll)
                        .child(
                            div()
                                .px(px(20.))
                                .py(px(16.))
                                .text_size(px(14.))
                                .line_height(px(22.))
                                .child(transcript::prose(
                                    crate::external_link::text_view(text).selectable(true),
                                    palette,
                                )),
                        )
                        .into_any_element(),
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod load_tests {
    use super::{AtomicBool, Ordering, WebEvent, loading_layers, takes_document_title, track_page_event};
    use crate::web::Loading;

    #[test]
    fn gateways_title_holds_over_the_documents() {
        // The recording: the header read the page's name, then "Page", the document's own <title>.
        assert!(!takes_document_title(true, "Page"));
        assert!(takes_document_title(false, "Exponential vs. polynomial growth"));
        assert!(!takes_document_title(false, "about:blank"));
        assert!(!takes_document_title(false, "  "));
    }

    #[test]
    fn the_outline_lies_under_the_page_until_it_shows_then_over_it() {
        assert_eq!(loading_layers(Some(&1), false), (Some(1), None));
        assert_eq!(loading_layers(Some(&1), true), (None, Some(1)));
        assert_eq!(loading_layers::<u8>(None, true), (None, None));
        assert_eq!(loading_layers::<u8>(None, false), (None, None));
    }

    fn loading(loading: bool) -> WebEvent {
        WebEvent::Loading(Loading {
            loading,
            back: false,
            forward: false,
        })
    }

    #[test]
    fn a_hidden_page_finishes_loading_without_visible_side_effects() {
        let loaded = AtomicBool::new(false);
        assert!(!track_page_event(
            &loading(false),
            Some("https://pages.test/document"),
            false,
            &loaded
        ));
        assert!(
            loaded.load(Ordering::Relaxed),
            "reopening this page must not report a timeout"
        );
        assert!(!track_page_event(
            &WebEvent::TitleChanged("Hidden".into()),
            None,
            false,
            &loaded
        ));
        assert!(!track_page_event(
            &WebEvent::LoadFailed(Some(502)),
            None,
            false,
            &loaded
        ));
        assert!(track_page_event(
            &WebEvent::TitleChanged("Shown".into()),
            None,
            true,
            &loaded
        ));
        assert!(loaded.load(Ordering::Relaxed));
    }

    #[test]
    fn blank_and_incomplete_loads_do_not_hide_a_real_timeout() {
        for (event, address) in [
            (loading(false), None),
            (loading(false), Some("about:blank")),
            (loading(true), Some("https://pages.test/document")),
        ] {
            let loaded = AtomicBool::new(false);
            assert!(track_page_event(&event, address, true, &loaded));
            assert!(!loaded.load(Ordering::Relaxed));
        }
        let loaded = AtomicBool::new(false);
        assert!(track_page_event(&WebEvent::LoadFailed(Some(502)), None, true, &loaded));
        assert!(!loaded.load(Ordering::Relaxed));
    }
}

#[cfg(test)]
#[path = "page_retry_tests.rs"]
mod retry_tests;
