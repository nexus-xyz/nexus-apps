//! The Inbox (`ui/src/components/nexus-ui/inbox/`): what needs the person in this organization, in three tabs (All,
//! Needs you, Unread). Gateway's notifications (`/v1/notifications`) and this computer's Code sessions that wait for
//! the person or failed share one list. A row opens what it is about: an artifact in the Workbench's Artifacts tab, a
//! Code session in the main area. The Nexus UI pages this build does not draw (admin pages, approvals, another
//! organization's artifact) open in the Workbench's Browser tab. The list is `inbox.rs`.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Place, View, Workspace, glyph};
use crate::code::Status;
use crate::inbox::{self, Entry, Filter, Item, Mark, Moment, Page, PageRequest, Target};
use crate::library::{self, chrono_lite};
use crate::motion;
use crate::palette::Palette;

/// The list's widest, as the Library's.
const MAX_WIDTH: f32 = 1080.;

/// What the Inbox shows.
pub(super) struct InboxState {
    volatile_scope: String,
    pub filter: Filter,
    /// Gateway's notifications, as read so far.
    pub items: Vec<Item>,
    pub next_cursor: Option<String>,
    /// How many pages are read; a refresh only replaces the list while it is one page.
    pub pages: usize,
    next_request: u64,
    reads: std::collections::HashMap<Option<String>, u64>,
    counted_pages: std::collections::HashSet<Option<String>>,
    /// Only the furthest requested page may advance pagination or finish Load more.
    frontier: Option<u64>,
    pub loaded: bool,
    pub loading_more: bool,
    /// This Gateway has no inbox for the person (404 or 403).
    pub unavailable: bool,
    /// Why the first read failed.
    pub error: Option<SharedString>,
    /// Why the last action did not work.
    pub note: Option<SharedString>,
    pub scroll: ScrollHandle,
    /// Groups the person opened: an account day's key, or the first item's id for other same-kind activity.
    pub expanded: std::collections::HashSet<String>,
    /// The row under the pointer, by its entry's id: its actions open left of its time.
    pub hovered: Option<String>,
}

impl InboxState {
    pub fn new() -> InboxState {
        InboxState {
            volatile_scope: uuid::Uuid::new_v4().to_string(),
            filter: Filter::All,
            items: Vec::new(),
            next_cursor: None,
            pages: 0,
            next_request: 0,
            reads: Default::default(),
            counted_pages: Default::default(),
            frontier: None,
            loaded: false,
            loading_more: false,
            unavailable: false,
            error: None,
            note: None,
            scroll: ScrollHandle::new(),
            expanded: Default::default(),
            hovered: None,
        }
    }
    fn begin(&mut self, cursor: Option<String>) -> PageRequest {
        self.next_request += 1;
        let request = PageRequest {
            id: self.next_request,
            cursor,
            unread: self.filter == Filter::Unread,
        };
        self.reads.insert(request.cursor.clone(), request.id);
        if request.cursor.is_some() || (self.pages <= 1 && !self.loading_more) {
            self.frontier = Some(request.id);
        }
        if request.cursor.is_some() {
            self.loading_more = true;
        }
        request
    }

    fn select_filter(&mut self, filter: Filter) {
        self.filter = filter;
        self.items.clear();
        self.next_cursor = None;
        self.pages = 0;
        self.reads.clear();
        self.counted_pages.clear();
        self.frontier = None;
        self.loaded = false;
        self.loading_more = false;
        self.error = None;
        self.note = None;
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    /// Read all, on screen at once: every loaded notification turns read. The ids that were unread.
    fn read_all(&mut self) -> Vec<String> {
        self.note = None;
        self.items
            .iter_mut()
            .filter(|item| !item.read)
            .map(|item| {
                item.read = true;
                item.id.clone()
            })
            .collect()
    }

    fn received(&mut self, request: &PageRequest, page: Result<Option<Page>, String>) {
        if request.unread != (self.filter == Filter::Unread) || self.reads.get(&request.cursor) != Some(&request.id) {
            return;
        }
        let append = request.cursor.is_some();
        let frontier = self.frontier == Some(request.id);
        if frontier {
            self.loading_more = false;
        }
        match page {
            Ok(None) if request.id == self.next_request => {
                self.unavailable = true;
                self.loaded = true;
                self.items.clear();
                self.next_cursor = None;
                self.pages = 0;
                self.reads.clear();
                self.counted_pages.clear();
                self.frontier = None;
                self.loading_more = false;
            }
            Ok(None) => {}
            Ok(Some(page)) => {
                self.unavailable = false;
                self.error = None;
                self.loaded = true;
                if append {
                    for item in page.items {
                        if let Some(held) = self.items.iter_mut().find(|held| held.id == item.id) {
                            *held = item;
                        } else {
                            self.items.push(item);
                        }
                    }
                } else if self.pages <= 1 {
                    self.items = page.items;
                } else {
                    let mut items = page.items;
                    for item in self.items.drain(..) {
                        if !items.iter().any(|fresh| fresh.id == item.id) {
                            items.push(item);
                        }
                    }
                    self.items = items;
                }
                self.counted_pages.insert(request.cursor.clone());
                self.pages = self.counted_pages.len();
                if frontier {
                    self.next_cursor = page.next_cursor;
                }
            }
            Err(error) if frontier => {
                if self.loaded {
                    if append {
                        self.note = Some(format!("Couldn't load more: {error}").into());
                    }
                } else {
                    self.error = Some(error.into());
                }
            }
            Err(_) => {}
        }
    }
}

impl Workspace {
    pub(super) fn session_notice_scope(&self) -> Option<String> {
        let account = self.account.as_ref()?;
        match &account.person_id {
            Some(person) => Some(serde_json::json!([self.gateway.url(), account.org, person]).to_string()),
            None => Some(format!(
                "volatile:{}",
                serde_json::json!([self.gateway.url(), account.org, self.inbox_state.volatile_scope])
            )),
        }
    }

    /// A loaded session reads its current completion, while pending requests still need an answer.
    pub(super) fn note_inbox_completion_seen(&self, cx: &mut App) {
        let View::Code(view) = &self.view else { return };
        if !view.loaded {
            return;
        }
        if let Some(entry) = self.inbox_entries(cx).into_iter().find(|entry| {
            entry.moment == Moment::Finished && !entry.read && entry.target == Some(Target::Code(view.id.clone()))
        }) {
            super::inbox_marks::mark(self.session_notice_scope().as_deref(), &entry.id, false, cx);
        }
    }

    pub(super) fn open_inbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        super::inbox_marks::ensure(self.session_notice_scope().as_deref(), cx);
        if !matches!(self.view, View::Inbox) {
            self.visit(Place::Inbox);
        }
        self.reset_chat();
        self.view = View::Inbox;
        self.title = "Inbox".into();
        // The list takes the keys at once, so ↑ ↓ and Ctrl+K work without a click.
        self.focus_page(window, cx);
        self.inbox_state.note = None;
        self.reload_inbox();
        cx.notify();
    }

    /// Read the first page again; what was read stays on screen until the answer.
    pub(super) fn reload_inbox(&mut self) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let request = self.inbox_state.begin(None);
        account_gateway.load_notifications(request);
    }

    pub(super) fn inbox_loaded(&mut self, request: PageRequest, page: Result<Option<Page>, String>) {
        self.inbox_state.received(&request, page);
    }

    pub(super) fn notification_marked(&mut self, item: Result<Item, String>) {
        match item {
            Ok(item) => {
                if let Some(held) = self.inbox_state.items.iter_mut().find(|held| held.id == item.id) {
                    *held = item;
                }
            }
            Err(error) => {
                self.inbox_state.note = Some(format!("That didn't save: {error}").into());
                self.reload_inbox();
            }
        }
        if let Some(account) = self.account_gateway() {
            account.load_inbox();
        }
    }

    pub(super) fn notifications_read(&mut self, items: Vec<Result<Item, String>>) {
        let mut failed = 0;
        let mut first_error = None;
        for item in items {
            match item {
                Ok(item) => {
                    if let Some(held) = self.inbox_state.items.iter_mut().find(|held| held.id == item.id) {
                        *held = item;
                    }
                }
                Err(error) => {
                    failed += 1;
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            let what = if failed == 1 {
                "One item".to_string()
            } else {
                format!("{failed} items")
            };
            self.inbox_state.note = Some(format!("{what} didn't save: {error}").into());
        }
        // Pages not loaded yet changed too, and the Unread tab drops what is read.
        self.reload_inbox();
        if let Some(account) = self.account_gateway() {
            account.load_inbox();
        }
    }

    /// Read all: Gateway's notifications and this computer's session notices, in every tab.
    fn read_all_inbox(&mut self, cx: &mut Context<Self>) {
        let local: Vec<String> = self
            .inbox_entries_in(Filter::All, cx)
            .into_iter()
            .filter(|entry| !entry.remote && !entry.read)
            .map(|entry| entry.id)
            .collect();
        let local: Vec<&str> = local.iter().map(String::as_str).collect();
        super::inbox_marks::mark_each(self.session_notice_scope().as_deref(), &local, false, cx);
        let known = self.inbox_state.read_all();
        if !self.inbox_state.unavailable
            && let Some(account) = self.account_gateway()
        {
            account.read_all_notifications(known);
        }
        self.inbox = 0;
        cx.notify();
    }

    /// Mark a notification read, or put it away. The list changes at once; Gateway's answer follows.
    fn mark_notification(&mut self, id: &str, mark: Mark, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let state = &mut self.inbox_state;
        match mark {
            Mark::Read => {
                if let Some(item) = state.items.iter_mut().find(|item| item.id == id) {
                    item.read = true;
                }
            }
            Mark::Done => state.items.retain(|item| item.id != id),
        }
        state.note = None;
        account_gateway.mark_notification(id.to_string(), mark);
        cx.notify();
    }

    fn mark_inbox_entry(&mut self, id: &str, remote: bool, mark: Mark, cx: &mut Context<Self>) {
        if remote {
            self.mark_notification(id, mark, cx);
            return;
        }
        if !self
            .inbox_entries(cx)
            .iter()
            .any(|entry| entry.id == id && !entry.remote)
        {
            return;
        }
        super::inbox_marks::mark(self.session_notice_scope().as_deref(), id, mark == Mark::Done, cx);
        cx.notify();
    }

    fn set_inbox_filter(&mut self, filter: Filter, cx: &mut Context<Self>) {
        if self.inbox_state.filter == filter {
            return;
        }
        self.inbox_state.select_filter(filter);
        self.reload_inbox();
        cx.notify();
    }

    fn load_more_inbox(&mut self, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let state = &mut self.inbox_state;
        let Some(cursor) = state.next_cursor.clone() else {
            return;
        };
        if state.loading_more {
            return;
        }
        let request = state.begin(Some(cursor));
        account_gateway.load_notifications(request);
        cx.notify();
    }

    /// The rows: Gateway's notifications and the local Code sessions that need the person, for the tab.
    fn inbox_entries(&self, cx: &App) -> Vec<Entry> {
        self.inbox_entries_in(self.inbox_state.filter, cx)
    }

    fn inbox_entries_in(&self, filter: Filter, cx: &App) -> Vec<Entry> {
        let marks = super::inbox_marks::read(self.session_notice_scope().as_deref(), cx);
        let now = chrono::Local::now().timestamp_millis();
        let mut recent = Vec::new();
        let mut local = Vec::new();
        for project in &self.projects {
            for session in &project.sessions {
                if session.archived {
                    continue;
                }
                let machine_name = session
                    .machine
                    .as_deref()
                    .map(|id| {
                        self.servers
                            .iter()
                            .find(|server| server.id == id)
                            .map(|server| server.name.clone())
                            .or_else(|| {
                                self.cloud_machines
                                    .iter()
                                    .find(|machine| machine.id == id)
                                    .map(|machine| machine.name.clone())
                            })
                            .unwrap_or_else(|| "Remote machine".into())
                    })
                    .unwrap_or_else(|| "This computer".into());
                let context = format!("{} · {machine_name}", project.name);
                if matches!(session.status, Status::NeedsYou | Status::Failed) {
                    let mut entry = inbox::code_entry(
                        &session.id,
                        &session.title,
                        session.status == Status::Failed,
                        session.activity.listed_at.unwrap_or(session.updated_at),
                    );
                    entry.id = format!(
                        "code:{}:{}:{}:{}",
                        session.machine.as_deref().unwrap_or("local"),
                        session.id,
                        if session.status == Status::Failed {
                            "error"
                        } else {
                            "waiting"
                        },
                        entry.at_ms.unwrap_or(0)
                    );
                    entry.context = context;
                    entry.needs_you = true;
                    local.push(entry);
                } else if let Some(at) = nexus_workbench::session_notices::completion(
                    session.activity.status,
                    session.archived,
                    session.created_at,
                    session.activity.listed_at,
                    now,
                ) {
                    recent.push(inbox::completion_entry(
                        &session.id,
                        &session.title,
                        session.machine.as_deref(),
                        context,
                        at,
                    ));
                }
            }
        }
        let apply = |entry: &mut Entry| {
            entry.read = marks.read(&entry.id, entry.needs_you, entry.at_ms.unwrap_or(0));
            marks.shown(&entry.id, entry.needs_you, entry.moment == Moment::Error)
        };
        local.retain_mut(apply);
        recent.retain_mut(apply);
        recent.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| a.id.cmp(&b.id)));
        recent.truncate(nexus_workbench::session_notices::MAX_RECENT);
        local.extend(recent);
        let remote = self.inbox_state.items.iter().map(inbox::entry);
        inbox::entries(local.into_iter().chain(remote).collect::<Vec<_>>(), filter)
    }

    /// Open what a row is about, and mark it read.
    fn open_inbox_entry(&mut self, entry: &Entry, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if !entry.read {
            self.mark_inbox_entry(&entry.id, entry.remote, Mark::Read, cx);
        }
        match &entry.target {
            Some(Target::Artifact(id)) => {
                let path = match &self.account {
                    Some(account) => format!("/app/{}/artifacts/{id}", account.org),
                    None => format!("/app?next=/artifacts/{id}"),
                };
                self.page_opening_auto = false;
                // The engine starts while Gateway answers with the page's address.
                crate::web::warm();
                account_gateway.frame_artifact(id.clone(), path);
            }
            Some(Target::Code(id)) => self.open_code(id, window, cx),
            Some(Target::Org(path)) if crate::pages::skills::Target::parse(path).is_some() => {
                let skills = crate::pages::skills::Target::parse(path).expect("checked by the guard");
                self.open_skills_target(skills, window, cx);
            }
            Some(target @ (Target::Viewer(_) | Target::Org(_) | Target::Approval(_))) => {
                // A Nexus UI page shows in the Workbench's Browser tab, not in the person's own browser.
                let org = self
                    .account
                    .as_ref()
                    .map(|account| account.org.clone())
                    .or_else(|| crate::desktop_settings::load().org);
                if let Some(url) = target.web_address(&self.gateway.site_url(), org.as_deref()) {
                    self.show_in_side_browser(&url, window, cx);
                }
            }
            None => {}
        }
        cx.notify();
    }

    pub(super) fn render_inbox(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.inbox_state;
        let filter = state.filter;
        let tabs = h_flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(8.))
            .bg(palette.hover)
            .children(Filter::ALL.map(|each| {
                h_flex()
                    .id(SharedString::from(format!("inbox-tab-{}", each.label())))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(each == filter, |chip| {
                        chip.bg(palette.selected).text_color(palette.strong)
                    })
                    .when(each != filter, |chip| {
                        chip.text_color(palette.muted)
                            .hover(|style| style.text_color(palette.strong))
                    })
                    .child(each.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.set_inbox_filter(each, cx)))
            }));
        let entries = self.inbox_entries(cx);
        // Read all covers every tab, so it shows while anything in the Inbox is unread.
        let any_unread = self.inbox > 0
            || if filter == Filter::All {
                entries.iter().any(|entry| !entry.read)
            } else {
                self.inbox_entries_in(Filter::All, cx).iter().any(|entry| !entry.read)
            };
        let body: AnyElement = if state.unavailable && entries.is_empty() {
            message(
                "The Inbox isn't on here yet",
                "This Gateway doesn't keep an inbox for you. Your Code sessions that need you still show here.",
                palette,
            )
            .into_any_element()
        } else if let (Some(error), true) = (&state.error, entries.is_empty()) {
            message(
                "Couldn't load the Inbox",
                &format!("{error} Try again in a moment."),
                palette,
            )
            .child(
                pill_button("inbox-retry", "Try again", palette)
                    .mt(px(8.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.inbox_state.error = None;
                        this.reload_inbox();
                        cx.notify();
                    })),
            )
            .into_any_element()
        } else if !state.loaded && entries.is_empty() {
            skeleton(palette).into_any_element()
        } else if entries.is_empty() {
            let (title, detail) = match filter {
                Filter::All => (
                    "You're all caught up",
                    "Questions, approvals and alerts for you show here.",
                ),
                Filter::Needs => (
                    "Nothing needs you",
                    "Approvals, questions and failures that wait on you show here.",
                ),
                Filter::Unread => ("No unread items", "Everything in your Inbox is read."),
            };
            message(title, detail, palette).into_any_element()
        } else {
            let now = chrono_lite::now();
            // Two sections, as Nexus UI's Inbox: what needs the person, then the rest. `entries` sorts needs first.
            let split = entries.partition_point(|entry| entry.needs_you);
            let needs = split;
            let mut first: Vec<AnyElement> = Vec::new();
            let mut rest: Vec<AnyElement> = Vec::new();
            let mut ix = 0;
            for mut group in inbox::presentation_groups(entries) {
                let target = if group.items[0].needs_you {
                    &mut first
                } else {
                    &mut rest
                };
                if group.items.len() < 2 {
                    target.push(
                        self.render_inbox_row(ix, group.items.remove(0), now, palette, cx)
                            .into_any_element(),
                    );
                    ix += 1;
                    continue;
                }
                let open = state.expanded.contains(&group.key);
                target.push(
                    self.render_inbox_group(ix, open, &group, palette, cx)
                        .into_any_element(),
                );
                ix += 1;
                if open {
                    for entry in group.items {
                        target.push(self.render_inbox_row(ix, entry, now, palette, cx).into_any_element());
                        ix += 1;
                    }
                }
            }
            v_flex()
                .gap(px(16.))
                .when(!first.is_empty(), |list| {
                    list.child(section("Needs you", Some(needs), palette).children(first))
                })
                .when(!rest.is_empty(), |list| {
                    list.child(section("Ready and recent", None, palette).children(rest))
                })
                .when(state.next_cursor.is_some(), |list| {
                    let loading = state.loading_more;
                    list.child(
                        h_flex().justify_center().child(
                            pill_button("inbox-more", if loading { "Loading…" } else { "Load more" }, palette)
                                .when(loading, |button| button.cursor_default().text_color(palette.faint))
                                .on_click(cx.listener(|this, _, _, cx| this.load_more_inbox(cx))),
                        ),
                    )
                })
                .into_any_element()
        };
        div().size_full().child(
            div()
                .id("inbox")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&state.scroll)
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(MAX_WIDTH))
                        // Centred in the main area, as every page there.
                        .mx_auto()
                        .px(px(48.))
                        .pt(px(32.))
                        .pb(px(48.))
                        .child(
                            v_flex()
                                .gap(px(4.))
                                .mb(px(24.))
                                .child(
                                    div()
                                        .text_size(px(28.))
                                        .line_height(px(34.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(palette.strong)
                                        .child("Inbox"),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(palette.faint)
                                        .child("What needs your attention in this organization."),
                                ),
                        )
                        .child(
                            h_flex()
                                .mb(px(16.))
                                .justify_between()
                                .child(tabs)
                                .when(any_unread, |row| {
                                    row.child(
                                        pill_button("inbox-read-all", "Read all", palette)
                                            .on_click(cx.listener(|this, _, _, cx| this.read_all_inbox(cx))),
                                    )
                                }),
                        )
                        .when_some(state.note.clone(), |column, note| {
                            column.child(
                                div()
                                    .mb(px(8.))
                                    .text_size(px(13.))
                                    .text_color(palette.destructive)
                                    .child(note),
                            )
                        })
                        .child(body),
                ),
        )
    }

    /// A daily account group or same-kind activity row; a click reveals every original event and its actions.
    fn render_inbox_group(
        &self,
        ix: usize,
        open: bool,
        group: &inbox::PresentationGroup,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let count: u64 = group.items.iter().map(|entry| entry.count.max(1)).sum();
        let head = &group.items[0];
        let title = if head.moment == Moment::Security && !head.needs_you {
            group.title.clone()
        } else {
            match head.title.strip_suffix(" a call") {
                Some(stem) => format!("{stem} {count} calls"),
                None => format!("{} ×{count}", head.title),
            }
        };
        let unread = group.items.iter().any(|entry| !entry.read);
        let icon_color = match head.moment {
            Moment::Error => palette.destructive,
            _ if head.needs_you => palette.strong,
            _ => palette.icon,
        };
        let key = group.key.clone();
        h_flex()
            .id(("inbox-group", ix))
            .tab_index(0)
            .w_full()
            .min_h(px(56.))
            .pr(px(12.))
            .rounded(px(8.))
            .text_size(px(13.))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .focus_visible(|style| style.bg(palette.hover))
            .active(|style| style.bg(palette.selected))
            .child(
                h_flex().w(px(22.)).flex_none().justify_center().child(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .when(unread, |dot| dot.bg(palette.strong)),
                ),
            )
            .child(
                div()
                    .w(px(16.))
                    .mr(px(12.))
                    .flex_none()
                    .child(glyph(moment_icon(head.moment), icon_color)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .text_color(if unread { palette.strong } else { palette.muted })
                            .when(unread, |title| title.font_weight(FontWeight::MEDIUM))
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(head.context.clone()),
                    ),
            )
            .child(super::disclosure("disclosure", open, palette.faint))
            .on_click(cx.listener(move |this, _, _, cx| {
                let expanded = &mut this.inbox_state.expanded;
                if !expanded.remove(&key) {
                    expanded.insert(key.clone());
                }
                cx.notify();
            }))
    }

    fn render_inbox_row(
        &self,
        ix: usize,
        entry: Entry,
        now: chrono_lite::Seconds,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let icon_color = match entry.moment {
            Moment::Error => palette.destructive,
            _ if entry.needs_you => palette.strong,
            _ => palette.icon,
        };
        // On hover the row's actions open left of its time: their width eases in, the title truncates into the room
        // they take, and the time stays where it is. A Gateway item offers Mark read and Done; a Code session leaves
        // the Inbox when it runs again, so it shows what a click on the row does (Approve, Open).
        let is_gateway = entry.remote;
        let (can_read, can_done) = entry.hover_actions();
        let hovered = self.inbox_state.hovered.as_deref() == Some(entry.id.as_str());
        let buttons = h_flex()
            .gap(px(4.))
            .when(can_read, |row| {
                let id = entry.id.clone();
                row.child(
                    pill_button(("inbox-read", ix), "Mark read", palette).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.mark_inbox_entry(&id, is_gateway, Mark::Read, cx);
                        },
                    )),
                )
            })
            .when(can_done, |row| {
                let id = entry.id.clone();
                row.child(pill_button(("inbox-done", ix), "Done", palette).on_click(cx.listener(
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.mark_inbox_entry(&id, is_gateway, Mark::Done, cx);
                    },
                )))
            });
        let opens = entry.target.is_some();
        // Completed sessions open from the row without an extra action pill.
        let pill = (!is_gateway && opens && entry.moment != Moment::Finished).then(|| {
            h_flex()
                .h(px(24.))
                .px(px(10.))
                .rounded_full()
                .bg(palette.selected)
                .text_size(px(12.))
                .text_color(palette.strong)
                .child(entry.action)
        });
        let has_actions = pill.is_some() || can_read || can_done;
        // 12 px from the text; the time keeps its own 12 px.
        let actions = has_actions.then(|| {
            h_flex()
                .pl(px(12.))
                .gap(px(4.))
                .children(pill)
                .child(buttons)
                .into_any_element()
        });
        let open = entry.clone();
        let hover_id = entry.id.clone();
        h_flex()
            .id(("inbox-row", ix))
            .tab_index(0)
            .when(has_actions, |row| {
                row.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    let state = &mut this.inbox_state;
                    if *hovered {
                        state.hovered = Some(hover_id.clone());
                    } else if state.hovered.as_deref() == Some(hover_id.as_str()) {
                        state.hovered = None;
                    } else {
                        return;
                    }
                    cx.notify();
                }))
            })
            .w_full()
            .min_h(px(56.))
            .pr(px(12.))
            .rounded(px(8.))
            .text_size(px(13.))
            .when(opens, |row| row.cursor_pointer())
            .hover(|style| style.bg(palette.hover))
            .focus_visible(|style| style.bg(palette.hover))
            .active(|style| style.bg(palette.selected))
            // The unread dot sits in the gutter, so read and unread titles start at the same x.
            .child(
                h_flex().w(px(22.)).flex_none().justify_center().child(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .when(!entry.read, |dot| dot.bg(palette.strong)),
                ),
            )
            .child(
                div()
                    .w(px(16.))
                    .mr(px(12.))
                    .flex_none()
                    .child(glyph(moment_icon(entry.moment), icon_color)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .min_w(px(0.))
                            .gap(px(4.))
                            .child(
                                div()
                                    .truncate()
                                    .text_color(if entry.read { palette.muted } else { palette.strong })
                                    .when(!entry.read, |title| title.font_weight(FontWeight::MEDIUM))
                                    .child(entry.title.clone()),
                            )
                            .when(entry.count > 1, |title| {
                                title.child(
                                    div()
                                        .flex_none()
                                        .text_color(palette.faint)
                                        .child(format!("×{}", entry.count)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(entry.context.clone()),
                    ),
            )
            .when_some(actions, |row, actions| {
                row.child(motion::reveal_width(
                    SharedString::from(format!("inbox-actions-{}", entry.id)),
                    hovered,
                    Some(actions),
                ))
            })
            // The time never hides: rows keep one right edge while the actions come and go.
            .child(
                div()
                    .flex_none()
                    .ml(px(12.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(ago(&entry, now)),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.open_inbox_entry(&open, window, cx)))
    }
}

fn moment_icon(moment: Moment) -> Lucide {
    match moment {
        Moment::Approval => Lucide::Hand,
        Moment::Error => Lucide::TriangleAlert,
        Moment::Request => Lucide::Users,
        Moment::Billing => Lucide::CreditCard,
        Moment::Security => Lucide::ShieldCheck,
        Moment::Integration => Lucide::Plug,
        Moment::Activity => Lucide::Bell,
        Moment::Finished => Lucide::Check,
    }
}

/// "now", "5m", "2h", "3d", then the day, as the Library's Updated column.
fn ago(entry: &Entry, now: chrono_lite::Seconds) -> String {
    if !entry.at.is_empty() {
        return library::short_ago(&entry.at, now);
    }
    let Some(ms) = entry.at_ms else {
        return String::new();
    };
    match (now - ms / 1000).max(0) {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// A section of the list: its title, with a count for "Needs you", over its rows.
fn section(title: &'static str, count: Option<usize>, palette: Palette) -> Div {
    v_flex().child(
        h_flex()
            .h(px(32.))
            // Lined up with the tabs and the title above.
            .pl(px(0.))
            .gap(px(8.))
            .text_size(px(12.))
            .text_color(palette.faint)
            .child(title)
            .when_some(count, |head, count| {
                head.child(
                    div()
                        .px(px(6.))
                        .rounded_full()
                        .bg(palette.hover)
                        .text_size(px(11.))
                        .text_color(palette.muted)
                        .child(count.to_string()),
                )
            }),
    )
}

/// A small filled button: no outline, the fill steps up on hover.
fn pill_button(id: impl Into<ElementId>, text: &'static str, palette: Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .flex_none()
        .h(px(24.))
        .px(px(10.))
        .rounded_full()
        .bg(palette.selected)
        .text_size(px(12.))
        .text_color(palette.foreground)
        .cursor_pointer()
        .hover(|style| style.bg(palette.border).text_color(palette.strong))
        .active(|style| style.opacity(0.85))
        .child(text)
}

fn message(title: &str, detail: &str, palette: Palette) -> Div {
    v_flex()
        .items_center()
        .py(px(64.))
        .gap(px(8.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child(title.to_string()),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child(detail.to_string()),
        )
}

/// Five grey rows the size of the list while the first read runs.
fn skeleton(palette: Palette) -> Div {
    v_flex().children((0..5).map(|ix| {
        h_flex()
            .min_h(px(56.))
            .gap(px(12.))
            .pl(px(24.))
            .pr(px(12.))
            .child(div().size(px(16.)).rounded(px(4.)).bg(palette.hover))
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .h(px(10.))
                            .w(px(200. + (ix % 3) as f32 * 60.))
                            .rounded(px(4.))
                            .bg(palette.hover),
                    )
                    .child(div().h(px(8.)).w(px(140.)).rounded(px(4.)).bg(palette.hover)),
            )
    }))
}

#[cfg(test)]
mod pagination_tests {
    use super::*;
    use core::prelude::v1::test;

    fn page(items: &[(&str, &str)], next: Option<&str>) -> Page {
        Page {
            items: items
                .iter()
                .map(|(id, title)| Item {
                    id: (*id).into(),
                    kind: "session_finished".into(),
                    group: "my_sessions".into(),
                    subject_text: (*title).into(),
                    subject_type: "session".into(),
                    subject_id: (*id).into(),
                    sources: Vec::new(),
                    count: 1,
                    read: false,
                    updated_at: "2026-11-10T00:00:00Z".into(),
                })
                .collect(),
            next_cursor: next.map(str::to_owned),
        }
    }
    fn answer(state: &mut InboxState, request: &PageRequest, items: &[(&str, &str)], next: Option<&str>) {
        state.received(request, Ok(Some(page(items, next))));
    }
    fn first_page() -> InboxState {
        let mut state = InboxState::new();
        let first = state.begin(None);
        answer(&mut state, &first, &[("first", "First")], Some("a"));
        state
    }

    #[test]
    fn cached_a_then_b_then_late_fresh_a_keeps_the_furthest_cursor_and_page_count() {
        let mut state = first_page();
        let a = state.begin(Some("a".into()));
        answer(&mut state, &a, &[("a", "Cached A")], Some("b"));
        let b = state.begin(Some("b".into()));
        answer(&mut state, &b, &[("b", "Cached B")], Some("c"));
        answer(&mut state, &a, &[("a", "Fresh A")], Some("stale-b"));
        assert_eq!(state.next_cursor.as_deref(), Some("c"));
        assert_eq!(state.pages, 3);
        assert!(!state.loading_more);
        assert_eq!(
            state
                .items
                .iter()
                .find(|item| item.id == "a")
                .map(|item| item.subject_text.as_str()),
            Some("Fresh A")
        );
        assert!(state.items.iter().any(|item| item.id == "b"));
        answer(&mut state, &b, &[("b", "Fresh B")], Some("fresh-c"));
        assert_eq!(state.next_cursor.as_deref(), Some("fresh-c"));
        assert_eq!(state.pages, 3);
        assert_eq!(state.items.len(), 3);
    }

    #[test]
    fn an_earlier_page_refresh_does_not_finish_a_later_load_more() {
        let mut state = first_page();
        let a = state.begin(Some("a".into()));
        answer(&mut state, &a, &[("a", "Cached A")], Some("b"));
        let b = state.begin(Some("b".into()));
        assert!(state.loading_more);
        answer(&mut state, &a, &[("a", "Fresh A")], Some("stale-b"));
        assert!(state.loading_more);
        assert_eq!(state.next_cursor.as_deref(), Some("b"));
        answer(&mut state, &b, &[("b", "B")], None);
        assert!(!state.loading_more);
        assert_eq!(state.next_cursor, None);
        assert_eq!(state.pages, 3);
    }

    #[test]
    fn a_superseded_first_page_read_cannot_replace_its_successor() {
        let mut state = InboxState::new();
        let old = state.begin(None);
        let current = state.begin(None);
        answer(&mut state, &current, &[("new", "New")], Some("a"));
        answer(&mut state, &old, &[("old", "Old")], Some("wrong"));
        assert_eq!(
            state.items.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["new"]
        );
        assert_eq!(state.next_cursor.as_deref(), Some("a"));
        assert_eq!(state.pages, 1);
    }

    #[test]
    fn switching_filters_away_and_back_invalidates_old_reads_of_the_same_filter() {
        let mut state = InboxState::new();
        let old_all = state.begin(None);
        state.select_filter(Filter::Unread);
        let unread = state.begin(None);
        state.select_filter(Filter::All);
        let all = state.begin(None);
        answer(&mut state, &old_all, &[("old", "Old All")], Some("wrong"));
        answer(&mut state, &unread, &[("unread", "Unread")], None);
        assert!(!state.loaded);
        assert!(state.items.is_empty());
        answer(&mut state, &all, &[("all", "Current All")], None);
        assert_eq!(state.items[0].id, "all");
        assert_eq!(state.pages, 1);
    }

    #[test]
    fn refreshing_the_first_page_preserves_loaded_tail_pages_and_their_cursor() {
        let mut state = first_page();
        let a = state.begin(Some("a".into()));
        answer(&mut state, &a, &[("a", "A")], Some("b"));
        let refresh = state.begin(None);
        answer(&mut state, &refresh, &[("first", "Updated first")], Some("new-a"));
        assert_eq!(state.next_cursor.as_deref(), Some("b"));
        assert_eq!(state.pages, 2);
        assert!(state.items.iter().any(|item| item.id == "a"));
        assert_eq!(state.items[0].subject_text, "Updated first");
    }

    #[test]
    fn retry_and_two_callbacks_count_a_cursor_only_once() {
        let mut state = first_page();
        let failed = state.begin(Some("a".into()));
        state.received(&failed, Err("offline".into()));
        assert!(!state.loading_more);
        assert_eq!(state.pages, 1);
        let retry = state.begin(Some("a".into()));
        answer(&mut state, &retry, &[("a", "Cached A")], Some("b"));
        state.received(&failed, Err("late failure".into()));
        answer(&mut state, &retry, &[("a", "Fresh A")], Some("fresh-b"));
        assert_eq!(state.pages, 2);
        assert_eq!(state.items.len(), 2);
        assert_eq!(state.next_cursor.as_deref(), Some("fresh-b"));
        assert!(!state.loading_more);
    }

    #[test]
    fn an_old_unavailable_reply_cannot_clear_a_newer_pending_page() {
        let mut state = first_page();
        let a = state.begin(Some("a".into()));
        answer(&mut state, &a, &[("a", "A")], Some("b"));
        let b = state.begin(Some("b".into()));
        state.received(&a, Ok(None));
        assert!(state.loading_more);
        assert!(!state.unavailable);
        assert_eq!(state.next_cursor.as_deref(), Some("b"));
        answer(&mut state, &b, &[("b", "B")], Some("c"));
        assert_eq!(state.pages, 3);
    }

    #[test]
    fn read_all_reads_every_loaded_item_and_names_only_the_unread() {
        let mut state = first_page();
        let a = state.begin(Some("a".into()));
        answer(&mut state, &a, &[("a", "A")], None);
        state.items[0].read = true;
        state.note = Some("old".into());
        assert_eq!(state.read_all(), ["a"]);
        assert!(state.items.iter().all(|item| item.read));
        assert_eq!(state.note, None);
        assert!(state.read_all().is_empty());
    }

    #[test]
    fn the_latest_unavailable_reply_invalidates_pending_reads() {
        let mut state = first_page();
        let a = state.begin(Some("a".into()));
        let refresh = state.begin(None);
        state.received(&refresh, Ok(None));
        answer(&mut state, &a, &[("a", "Stale A")], Some("b"));
        assert!(state.unavailable);
        assert!(state.items.is_empty());
        assert_eq!(state.next_cursor, None);
        assert_eq!(state.pages, 0);
        assert!(!state.loading_more);
    }
}
