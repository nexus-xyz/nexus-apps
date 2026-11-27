//! The open chat: its messages in one centered column, the composer under them, and the empty state of a new chat.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::text::{TextView, TextViewState, TextViewStyle};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{IconName, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::code_block;
pub(crate) use super::code_block::RunCommand;
use super::side_pane::Tool;
use super::{Workspace, glyph};
use crate::model::{Message, Meta, PageBlock, Role, Stored};
use crate::motion;
use crate::palette::Palette;

/// How close to the bottom still counts as reading the latest.
const NEAR_BOTTOM: f32 = 48.;

/// The chat's notice: a failure in red, but a managed setting nobody in the app can change as a quiet line with a lock.
fn render_notice(notice: SharedString, palette: Palette, cx: &App) -> AnyElement {
    if notice.as_ref() == super::org_menu::ONE_ORG {
        return crate::setup::quiet::info_line("chat:one-org", Lucide::Lock, notice, palette, cx).into_any_element();
    }
    div()
        .text_sm()
        .text_color(palette.destructive)
        .child(notice)
        .into_any_element()
}

/// How far past the visible part the transcript lays out messages, so a scroll does not show them pop in.
const OVERDRAW: Pixels = px(800.);

/// The open chat's messages as a list that lays out only the ones on screen: one item per message, then the tail (the
/// loading lines, a notice). A long chat laid out every message on each frame, the caret's blink included. The list
/// follows the end while the end shows, and a message that grows (a streaming answer) is measured again.
pub(super) struct ChatList {
    pub list: ListState,
    /// What each message looked like when it was last measured: its id and what changes its height.
    shape: Vec<(usize, u64)>,
    tail: u64,
    /// The minimap's hover and drag (`minimap.rs`).
    pub minimap: super::minimap::MinimapState,
}

impl ChatList {
    pub fn new() -> ChatList {
        let list = ListState::new(1, ListAlignment::Top, OVERDRAW);
        list.set_follow_mode(FollowMode::Tail);
        ChatList {
            list,
            shape: Vec::new(),
            tail: 0,
            minimap: Default::default(),
        }
    }

    /// Bring the list up to date with the messages: only the ones from the first that changed are measured again. A
    /// session opened or a question sent goes to the end, wherever the reader was.
    #[cfg(test)]
    fn sync(&mut self, entries: &[Entry], opening: bool, notice: &Option<SharedString>) {
        self.sync_open(entries, None, opening, notice);
    }

    /// [`ChatList::sync`], with `open`: the entry Rewind's confirmation or the editor is open on, and a hash of what
    /// it shows, which changes that entry's height too.
    fn sync_open(
        &mut self,
        entries: &[Entry],
        open: Option<(usize, u64)>,
        opening: bool,
        notice: &Option<SharedString>,
    ) {
        let shape: Vec<(usize, u64)> = entries
            .iter()
            .map(|entry| {
                let key = entry.height_key();
                match open {
                    Some((id, shown)) if id == entry.id => (entry.id, key ^ shown.rotate_left(1)),
                    _ => (entry.id, key),
                }
            })
            .collect();
        let tail = {
            let mut hasher = DefaultHasher::new();
            (opening, notice).hash(&mut hasher);
            hasher.finish()
        };
        let same = unchanged_prefix(&self.shape, &shape);
        if same == self.shape.len() && same == shape.len() && tail == self.tail {
            return;
        }
        // The tail is one item after the messages, so it is always among the ones measured again.
        self.list.splice(same..self.shape.len() + 1, shape.len() + 1 - same);
        let reopened = same == 0 && !shape.is_empty();
        let sent = shape.last().map(|last| last.0) != self.shape.last().map(|last| last.0)
            && entries.last().is_some_and(|entry| entry.arriving);
        if reopened || sent {
            self.list.set_follow_mode(FollowMode::Tail);
        }
        self.shape = shape;
        self.tail = tail;
    }
}

/// How many messages at the start are as they were: the list measures again from there on.
fn unchanged_prefix(old: &[(usize, u64)], new: &[(usize, u64)]) -> usize {
    old.iter().zip(new).take_while(|(old, new)| old == new).count()
}

/// One message in the open chat. An answer is Markdown in a text view that takes streamed text as it arrives.
pub(super) struct Entry {
    /// Unique in this window's life: the hover group, the copy check and the arrival animation key off it.
    pub id: usize,
    pub role: Role,
    pub text: SharedString,
    pub answer: Option<Entity<TextViewState>>,
    /// The answer's text so far, to copy it and to tell whether the stored answer differs.
    pub streamed: String,
    pub streaming: bool,
    pub stopped: bool,
    pub error: Option<SharedString>,
    /// Sent or started now, so it animates in; messages loaded with a session do not.
    pub arriving: bool,
    /// The page the answer wrote, shown as a card with Open under it.
    pub page: Option<PageBlock>,
    /// The answer is writing a page now, and it is not stored yet.
    pub writing_page: bool,
    /// The answer is searching the web now (`web_search`), until its next text.
    pub searching_web: bool,
    /// The web pages the answer's searches read, a chip in its footer (`web_sources.rs`).
    pub web_sources: Vec<nexus_desktop_core::chat::WebSource>,
    /// The sources list is open under the footer. Kept while the chat is open; a chat opened again starts closed.
    pub sources_open: bool,
    /// What Gateway told about the answer, for the quiet line under it; empty for an answer streamed in this window.
    pub meta: Meta,
    /// A question's sent or restored images and files, shown over its text and sent again by Retry.
    pub attachments: crate::attachments::Draft,
    pub attachment_load: super::chat_attachments::Load,
    /// Gateway's id, rewind mark and attachments: a question takes Rewind and Edit once it has an id, and a rewound
    /// message shows dimmed.
    pub stored: Stored,
}

impl Entry {
    pub fn user(id: usize, text: String) -> Entry {
        Entry {
            id,
            role: Role::User,
            text: text.into(),
            answer: None,
            streamed: String::new(),
            streaming: false,
            stopped: false,
            error: None,
            arriving: false,
            page: None,
            writing_page: false,
            searching_web: false,
            web_sources: Vec::new(),
            sources_open: false,
            meta: Meta::default(),
            attachments: Default::default(),
            attachment_load: Default::default(),
            stored: Stored::default(),
        }
    }

    pub fn answer(id: usize, markdown: &str, streaming: bool, cx: &mut App) -> Entry {
        Entry {
            id,
            role: Role::Assistant,
            text: SharedString::default(),
            answer: Some(cx.new(|cx| TextViewState::markdown(markdown, cx))),
            streamed: markdown.to_string(),
            streaming,
            stopped: false,
            error: None,
            arriving: false,
            page: None,
            writing_page: false,
            searching_web: false,
            web_sources: Vec::new(),
            sources_open: false,
            meta: Meta::default(),
            attachments: Default::default(),
            attachment_load: Default::default(),
            stored: Stored::default(),
        }
    }

    pub fn from_message(id: usize, message: Message, cx: &mut App) -> Entry {
        match message.role {
            Role::User => {
                // The artifacts it referenced show as chips, not as the block the agent read.
                let (text, refs) = crate::artifact_refs::split(&message.content);
                let mut attachments = crate::attachments::Draft::default();
                let _ = attachments.add_artifacts(
                    refs.into_iter()
                        .map(|reference| crate::attachments::Attachment::artifact(reference, None))
                        .collect(),
                );
                Entry {
                    stored: message.stored,
                    attachments,
                    ..Entry::user(id, text.to_string())
                }
            }
            Role::Assistant => Entry {
                page: message.page,
                meta: message.meta,
                web_sources: message.web_sources,
                stored: message.stored,
                ..Entry::answer(id, &message.content, false, cx)
            },
        }
    }

    pub fn arriving(mut self) -> Entry {
        self.arriving = true;
        self
    }

    pub fn with_attachments(mut self, attachments: crate::attachments::Draft) -> Entry {
        self.attachments = attachments;
        self
    }

    /// A hash of what sets this message's height: the list measures it again when it changes.
    fn height_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        (
            self.streamed.len(),
            self.text.len(),
            self.streaming,
            self.stopped,
            &self.error,
            self.writing_page,
            self.searching_web,
            // The sources list open or closed changes the message's height too.
            (self.web_sources.len(), self.sources_open),
            self.page.as_ref().map(|page| (&page.page_id, page.version)),
            self.meta.line(),
            (self.attachments.images.len(), self.attachments.files.len()),
            &self.attachment_load,
        )
            .hash(&mut hasher);
        hasher.finish()
    }

    fn copy_text(&self) -> String {
        match self.role {
            Role::User => self.text.to_string(),
            Role::Assistant => self.streamed.clone(),
        }
    }
}

/// Give the answers Gateway's meta (`model::meta_for`); whether any changed.
pub(super) fn fill_meta(entries: &mut [Entry], stored: &[Message]) -> bool {
    let local: Vec<(Role, bool)> = entries
        .iter()
        .map(|entry| (entry.role, entry.meta != Meta::default()))
        .collect();
    let mut changed = false;
    for (entry, meta) in entries.iter_mut().zip(crate::model::meta_for(&local, stored)) {
        if let Some(meta) = meta {
            entry.meta = meta;
            changed = true;
        }
    }
    changed
}

/// The card while an answer writes a page, as Nexus UI's `WritingPage`.
fn render_writing_page(palette: Palette) -> impl IntoElement {
    h_flex()
        .max_w(px(460.))
        .gap(px(12.))
        .mt(px(8.))
        .py(px(10.))
        .px(px(14.))
        .rounded(px(12.))
        .bg(palette.raised)
        .child(glyph(Lucide::FileText, palette.icon).size(px(18.)))
        .child(
            div()
                .text_size(px(13.))
                .text_color(palette.muted)
                .child("Writing page…"),
        )
}

/// Whether the transcript shows its end (as of the last frame), or does not scroll at all.
pub(super) fn near_bottom(scroll: &ScrollHandle) -> bool {
    let max = scroll.max_offset().y;
    max <= px(NEAR_BOTTOM) || -scroll.offset().y >= max - px(NEAR_BOTTOM)
}

impl Workspace {
    /// Copy a message's text as it is now: the question, or the answer so far.
    pub(super) fn copy_entry(&mut self, id: usize, cx: &mut Context<Self>) {
        if let Some(text) = self.entries.iter().find(|entry| entry.id == id).map(Entry::copy_text) {
            self.copy(id, text, cx);
        }
    }

    pub(super) fn render_chat(&mut self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.entries.is_empty() && !self.opening;
        if self.find.open && empty {
            self.find.open = false;
        }
        self.refresh_find(cx);
        let open = self.chat_rewind.as_ref().map(|open| {
            let mut hasher = DefaultHasher::new();
            let lines = open
                .editor
                .as_ref()
                .map(|(input, _)| input.read(cx).value().lines().count());
            (lines, open.busy, &open.error).hash(&mut hasher);
            (open.entry, hasher.finish())
        });
        self.chat_list
            .sync_open(&self.entries, open, self.opening, &self.notice);
        let chat = v_flex().flex_1().min_w(px(0.)).h_full();
        if empty {
            // A new chat: the launch head and the composer together in the middle, as the Nexus app's new session with
            // No project picked (`new_code.rs`).
            let heading = self.render_launch_head(palette).into_any_element();
            // The art behind the window marks this screen, and its "Background" button is last, over the column, so it
            // takes its clicks.
            let (art, background) = self.render_backdrop(palette, cx);
            let chat = chat.relative().child(art);
            let chat = chat.child(
                v_flex()
                    .flex_1()
                    .justify_center()
                    .items_center()
                    .px(px(24.))
                    .py(px(48.))
                    .gap(px(20.))
                    .child(motion::arrive(div().child(heading), "empty-heading", 6.))
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(self.chat_widths().composer))
                            .gap(px(8.))
                            .child(self.render_composer(palette, cx))
                            // The where line under the box, as on Code's Home: "No project" and where it runs.
                            .child(self.render_where_line(true, palette, cx)),
                    )
                    .children(self.render_starters(palette, cx))
                    .when_some(self.notice.clone(), |column, notice| {
                        column.child(render_notice(notice, palette, cx))
                    }),
            );
            return chat.child(background);
        }

        // The new chat's screen leaving under the chat just started (`launch_motion.rs`).
        let chat = chat.relative().children(self.render_launch_exit(palette, cx));
        // The transcript is a cached view of its own (`pane.rs`): a caret blink in the composer leaves it be. The
        // composer floats over its foot.
        chat.child(
            div()
                .relative()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .child(self.render_transcript_pane(super::pane::Region::ChatTranscript))
                .child({
                    let dock = div()
                        .w_full()
                        .max_w(px(self.composer_width()))
                        .mx_auto()
                        .px(px(24.))
                        // The Nexus app keeps its composer about 30px off the window's foot.
                        .pb(px(28.))
                        .children(self.render_move_here(palette, cx))
                        .child(self.render_composer(palette, cx));
                    self.floating_dock(dock, palette, cx)
                }),
        )
    }

    /// The chat's transcript: the list of messages, the fade under the title, the minimap, the find bar and the jump
    /// to the latest. Drawn in its own pane (`pane.rs`).
    pub(super) fn render_chat_scroll(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let away = !self.chat_list.list.is_following_tail() && !self.opening;
        let state = self.chat_list.list.clone();
        div()
            .relative()
            .flex_1()
            .min_h(px(0.))
            .child(
                list(
                    self.chat_list.list.clone(),
                    cx.processor(|this: &mut Self, ix: usize, window, cx| {
                        let palette = Palette::for_choice(this.theme, window.appearance());
                        this.render_chat_item(ix, palette, cx)
                    }),
                )
                .size_full(),
            )
            .child(top_fade(palette))
            .child(self.render_chat_minimap(palette, cx))
            .when(self.find.open, |area| area.child(self.render_find_bar(palette, cx)))
            .when_some(motion::presence("chat-jump", away, cx), |area, phase| {
                area.child(over_dock(self.dock_room(), render_list_jump(state, phase, palette, cx)))
            })
            .into_any_element()
    }

    /// One item of the chat's list, in the centered column: a message, or the tail after the last one.
    fn render_chat_item(&self, ix: usize, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let column = v_flex().w_full().max_w(px(self.column_width())).mx_auto().px(px(24.));
        let top = px(if ix == 0 { 28. } else { 24. });
        let item = match self.entries.get(ix) {
            Some(entry) => {
                let last = ix + 1 == self.entries.len();
                column.pt(top).child(self.render_entry(entry, last, palette, cx))
            }
            None => {
                let loading = self.opening;
                let notice = self.notice.clone();
                // Room for the composer floating over the list's foot.
                let column = column.pb(px(36.) + self.dock_room());
                if !loading && notice.is_none() {
                    column
                } else {
                    column.pt(top).child(
                        v_flex()
                            .gap(px(24.))
                            .when(loading, |column| column.child(render_loading(palette)))
                            .when_some(notice, |column, notice| {
                                column.child(render_notice(notice, palette, cx))
                            }),
                    )
                }
            }
        };
        div().w_full().child(item).into_any_element()
    }

    /// The round arrow over the transcript's foot when the reader scrolled up: back to the latest.
    pub(super) fn render_jump(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let button = div()
            .id("jump-to-latest")
            .size(px(32.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(palette.pop)
            .border_1()
            .border_color(palette.border)
            .shadow_md()
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .active(|style| style.opacity(0.8))
            .on_click(cx.listener(|this, _, _, cx| {
                this.scroll.scroll_to_bottom();
                cx.notify();
            }))
            .child(glyph(IconName::ArrowDown, palette.foreground));
        h_flex()
            .absolute()
            .bottom(px(12.))
            .left_0()
            .right_0()
            .justify_center()
            .child(motion::fade_in(button, "jump-to-latest-in"))
    }

    /// What a link in an answer does: a Nexus page opens in the Workbench's Artifacts tab, as in a Code session; any
    /// other link opens in the browser.
    fn link_opener(&self) -> impl Fn(&SharedString, &ClickEvent, &mut Window, &mut App) + Send + Sync + 'static {
        let gateway = self.account_gateway();
        let org = self.account.as_ref().map(|account| account.org.clone());
        move |href, _, _, cx| match crate::library::page_link(href) {
            Some(id) if let Some(gateway) = &gateway => {
                let path = match &org {
                    Some(org) => format!("/app/{org}/pages/{id}"),
                    None => format!("/app?next=/pages/{id}"),
                };
                crate::web::warm();
                gateway.frame_page(id, path);
            }
            _ => crate::external_link::open(href, cx),
        }
    }

    /// Open the page a chat answer wrote, at the version it names, in the Workbench's Artifacts tab. `auto`: the app
    /// opens it on its own as the answer ends, not the person.
    pub(super) fn open_page_block(&mut self, page: &PageBlock, auto: bool, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let id = page.page_id.clone();
        let path = match self.account.as_ref().map(|account| account.org.clone()) {
            Some(org) => format!("/app/{org}/pages/{id}"),
            None => format!("/app?next=/pages/{id}"),
        };
        self.page_opening = Some((id.clone(), page.version));
        // A version the person had open lately is still loaded: it shows at once, without asking Gateway.
        if !auto && self.show_kept_page(&format!("{id}@{}", page.version), cx) {
            return;
        }
        // The engine starts while Gateway answers with the page's address.
        crate::web::warm();
        self.page_opening_auto = auto;
        account_gateway.frame_page_version(id, page.version, path);
        cx.notify();
    }

    /// Whether the Artifacts tab holds this page, at this version, on screen or not.
    pub(super) fn page_block_loaded(&self, key: &(String, u32)) -> bool {
        self.page
            .as_ref()
            .is_some_and(|panel| panel.block.as_ref() == Some(key))
    }

    /// Whether a page's card reads "Opened": only while the page is on screen. Hidden, its Open shows it again.
    pub(super) fn page_card_open(&self, key: &(String, u32)) -> bool {
        self.page_block_loaded(key) && self.side.showing(Tool::Artifacts)
    }

    /// A page card's Open: show the page again when the Artifacts tab still holds it (the Workbench closed, or another
    /// tab on top), without loading it again; else `load` it.
    pub(super) fn show_or_load_page(
        &mut self,
        key: &(String, u32),
        window: &mut Window,
        cx: &mut Context<Self>,
        load: impl FnOnce(&mut Self, &mut Context<Self>),
    ) {
        if self.page_block_loaded(key) {
            self.pick_tool(Tool::Artifacts, window, cx);
        } else {
            load(self, cx);
        }
    }

    /// The card under an answer that wrote a page, as Nexus UI's `PageBlock`: its title, version and format, and Open.
    fn render_page_card(
        &self,
        entry: usize,
        page: &PageBlock,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let key = (page.page_id.clone(), page.version);
        let open = self.page_card_open(&key);
        let opening = self.page_opening.as_ref() == Some(&key);
        let icon = if page.format == "md" {
            Lucide::FileText
        } else {
            Lucide::AppWindow
        };
        let block = page.clone();
        // The page's address too: Gateway answers while the engine starts, and the click only loads it.
        let (gateway, id, version) = (self.account_gateway(), page.page_id.clone(), page.version);
        Self::page_card(
            ("page-open", entry),
            icon,
            (page.title.clone().into(), page.facts().into()),
            (open, opening),
            palette,
            move |hovered, window, cx| {
                let Some(gateway) = gateway.clone() else { return };
                let id = id.clone();
                crate::web::prepare_on_hover_then(super::page_canvas::STARTUP, *hovered, window, cx, move || {
                    gateway.prefetch_frame_page_version(id, version)
                })
            },
            cx.listener(move |this, _, window, cx| {
                let key = (block.page_id.clone(), block.version);
                this.show_or_load_page(&key, window, cx, |this, cx| this.open_page_block(&block, false, cx));
            }),
        )
    }

    /// A page's card: its icon, `(title, quiet line)`, and Open, which reads "Opening…" and then "Opened" (`(open,
    /// opening)`). Chat's answers and Code sessions (`code_pages.rs`) show it. `on_hover` runs as the pointer enters
    /// and leaves the card.
    pub(super) fn page_card(
        id: impl Into<ElementId>,
        icon: Lucide,
        (title, note): (SharedString, SharedString),
        (open, opening): (bool, bool),
        palette: Palette,
        on_hover: impl Fn(&bool, &mut Window, &mut App) + 'static,
        on_open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let id: ElementId = id.into();
        let label = if open {
            "Opened"
        } else if opening {
            "Opening…"
        } else {
            "Open"
        };
        h_flex()
            .id(ElementId::NamedChild(std::sync::Arc::new(id.clone()), "card".into()))
            // A pointer resting on the card: the person is likely to open it, so the web engine starts. A card that
            // scrolls under a still pointer hovers too: `warm_on_hover` waits for the pointer to rest.
            .on_hover(on_hover)
            .max_w(px(460.))
            .gap(px(12.))
            .mt(px(8.))
            .py(px(10.))
            .pl(px(14.))
            .pr(px(10.))
            .rounded(px(12.))
            .bg(palette.raised)
            .child(glyph(icon, palette.icon).size(px(18.)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.foreground)
                            .child(title),
                    )
                    .child(div().text_size(px(12.)).text_color(palette.muted).child(note)),
            )
            .child(
                div()
                    .id(id)
                    .flex_none()
                    .h(px(28.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .rounded(px(8.))
                    .text_size(px(13.))
                    .bg(if open { palette.selected } else { palette.hover })
                    .text_color(palette.foreground)
                    .when(!open && !opening, |button| {
                        button
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.selected))
                            .active(|style| style.opacity(0.8))
                            .on_click(on_open)
                    })
                    .child(label),
            )
    }

    /// A question's text, with the find bar's matches marked.
    fn question_text(&self, entry: &Entry) -> AnyElement {
        let (marks, current) = self.find.marks_in(entry.id);
        if !self.find.open || marks.is_empty() {
            return super::message_text::MessageText::new(("question-text", entry.id), entry.text.clone())
                .into_any_element();
        }
        let (plain, strong) = super::find_bar::mark_colors();
        let highlights = marks.iter().map(|range| {
            let color = if current.as_ref() == Some(range) { strong } else { plain };
            (
                range.clone(),
                HighlightStyle {
                    background_color: Some(color),
                    ..Default::default()
                },
            )
        });
        super::message_text::MessageText::new(("question-text", entry.id), entry.text.clone())
            .with_highlights(highlights)
            .into_any_element()
    }

    /// A question as sent or restored: its images and files, then its text in the bubble.
    fn render_sent_question(&self, column: Div, entry: &Entry, palette: Palette, cx: &mut Context<Self>) -> Div {
        column
            .children(self.render_attachment_load(entry, palette, cx))
            .when(!entry.attachments.is_empty(), |column| {
                // A sent image opens full size among the message's images.
                let images = entry.attachments.images.clone();
                let workspace = cx.weak_entity();
                let open: Rc<dyn Fn(usize, &mut Window, &mut App)> = Rc::new(move |index, _, cx| {
                    let _ = workspace.update(cx, |this, cx| this.open_image(&images, index, cx));
                });
                column.child(super::composer::artifact_chips(
                    format!("sent-{}", entry.id),
                    &entry.attachments,
                    palette,
                    None,
                    None,
                    Some(open),
                    Some(self.artifact_opener(cx)),
                ))
            })
            .when(!entry.text.is_empty(), |column| {
                column.child(
                    // `.msg-user`: no ring, the bubble tone, 14/22.
                    div()
                        .relative()
                        .max_w(relative(0.8))
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(12.))
                        .bg(palette.bubble)
                        .text_size(px(14.))
                        .line_height(px(22.))
                        .child(self.question_text(entry)),
                )
            })
    }

    fn render_entry(&self, entry: &Entry, last: bool, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let group: SharedString = format!("entry-{}", entry.id).into();
        let copied = self.copied == Some(entry.id);
        let copy = {
            let id = entry.id;
            div()
                .id(("copy", entry.id))
                .size(px(28.))
                .rounded(px(8.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .active(|style| style.opacity(0.7))
                .tooltip(|window, cx| Tooltip::new("Copy").build(window, cx))
                // The text is read on click, not copied on every frame for every message.
                .on_click(cx.listener(move |this, _, _, cx| this.copy_entry(id, cx)))
                .child(glyph(
                    if copied { IconName::Check } else { IconName::Copy },
                    palette.faint,
                ))
        };
        // Ask the last question again (`RotateCcw`, "Ask again"), under the latest answer only.
        let again = div()
            .id(("again", entry.id))
            .size(px(28.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .active(|style| style.opacity(0.7))
            .tooltip(|window, cx| Tooltip::new("Ask again").build(window, cx))
            .on_click(cx.listener(|this, _, _, cx| this.retry(cx)))
            .child(glyph(gpui_kit::assets::IconName::RotateCcw, palette.faint));
        // The row of actions under a message shows on hover; under the latest answer it stays.
        let actions = |always: bool| {
            h_flex().gap(px(2.)).when(!always && !copied, |row| {
                row.opacity(0.).group_hover(group.clone(), |style| style.opacity(1.))
            })
        };

        let element = match entry.role {
            Role::User => {
                // Rewind's confirmation keeps the actions showing, as the Nexus app's open popover does.
                let confirming = self
                    .chat_rewind
                    .as_ref()
                    .is_some_and(|open| open.entry == entry.id && open.editor.is_none());
                let prompt_actions = self.render_prompt_actions(entry, palette, cx);
                let confirm = self.render_rewind_confirm(entry, palette, cx);
                let editor = self.render_prompt_editor(entry, palette, cx);
                let column = v_flex().group(group.clone()).w_full().items_end().gap(px(4.));
                let column = match editor {
                    Some(editor) => column.child(editor),
                    None => self.render_sent_question(column, entry, palette, cx),
                };
                column
                    .child(actions(confirming).children(prompt_actions).child(copy))
                    .children(confirm)
                    .into_any_element()
            }
            Role::Assistant => {
                let thinking = entry.streaming && entry.streamed.is_empty();
                let done = !entry.streaming && !entry.streamed.is_empty();
                v_flex()
                    .group(group.clone())
                    .w_full()
                    .gap(px(8.))
                    .when(thinking, |column| column.child(render_thinking(entry.id, palette)))
                    .when_some(entry.answer.as_ref().filter(|_| !thinking), |column, answer| {
                        column.child(
                            div().w_full().text_size(px(14.)).line_height(px(22.)).child(prose(
                                crate::external_link::text_view(answer)
                                    .markdown_extensions(crate::rich_text::extensions())
                                    .selectable(true)
                                    .stream_fade(true)
                                    .on_reveal({
                                        let list = self.chat_list.list.clone();
                                        move |bounds, _, _| super::find_bar::reveal_bounds(&list, bounds)
                                    })
                                    .on_link_click({
                                        let handler = self.context_link_handler(self.link_opener(), cx);
                                        move |url, event, window, cx| handler(url, event, window, cx)
                                    }),
                                palette,
                            )),
                        )
                    })
                    .when_some(entry.page.as_ref(), |column, page| {
                        column.child(self.render_page_card(entry.id, page, palette, cx))
                    })
                    .when(entry.writing_page && entry.page.is_none(), |column| {
                        column.child(render_writing_page(palette))
                    })
                    .when(entry.searching_web && entry.streaming, |column| {
                        column.child(super::chat_choices::render_searching_web(palette))
                    })
                    .when_some(entry.error.clone(), |column, error| {
                        // A budget or a limit is setup, never a red failure: its amber line (`limit_line.rs`).
                        if let Some(stop) = crate::limit_stop::stop_of(&error) {
                            let line = self.chat_limit_line(&stop, last, cx);
                            let fixer = self.fixer(cx);
                            return column.child(line.render(&fixer, palette, cx));
                        }
                        // A missing key is setup, not a failure: its line adds the key or asks who can (`no_key.rs`).
                        if let Some(provider) = crate::pay_notice::missing_key(&error) {
                            let line = self.chat_no_key_line(provider, last, cx);
                            let fixer = self.fixer(cx);
                            return column.child(line.render(&fixer, palette, cx));
                        }
                        column.child(
                            h_flex()
                                .gap(px(10.))
                                .text_sm()
                                .debug_selector(|| "chat-error-row".into())
                                .child(
                                    div()
                                        .min_w(px(0.))
                                        .text_color(palette.destructive)
                                        .debug_selector(|| "chat-error".into())
                                        .child(error.clone()),
                                )
                                .when(last, |row| {
                                    row.child(
                                        div()
                                            .id("retry")
                                            .flex_none()
                                            .debug_selector(|| "chat-retry".into())
                                            .text_color(palette.foreground)
                                            .cursor_pointer()
                                            .hover(|style| style.opacity(0.7))
                                            .on_click(cx.listener(|this, _, _, cx| this.retry(cx)))
                                            .child("Try again"),
                                    )
                                })
                                .child(code_block::copy_button(error, palette)),
                        )
                    })
                    .when(done || entry.stopped, |column| {
                        // The quiet line (Nexus UI's `.msg-foot`): the web sources chip, then model, cost and tokens,
                        // the exact time on hover, and the actions right after it. Nexus UI pushes them to the
                        // column's right edge, which in this wider column left them ~550px from the line they belong
                        // to; they keep its reveal rule (on hover, always under the latest answer). The chip is
                        // content, so it always shows; it opens the sources list in place, under the line.
                        let line = entry.meta.line().filter(|_| done);
                        let time = entry.meta.exact_time();
                        let has_sources = !entry.web_sources.is_empty();
                        let sources = has_sources.then(|| {
                            super::web_sources::chip(entry.id, &entry.web_sources, entry.sources_open, palette, cx)
                        });
                        let list = (has_sources && entry.sources_open)
                            .then(|| super::web_sources::list(entry.id, &entry.web_sources, palette, cx));
                        column.child(
                            v_flex()
                                .w_full()
                                .gap(px(4.))
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap(px(8.))
                                        .items_center()
                                        .children(sources)
                                        .when_some(line, |row, line| {
                                            row.child(
                                                div()
                                                    .id(("meta", entry.id))
                                                    .min_w(px(0.))
                                                    .truncate()
                                                    .text_size(px(12.))
                                                    .text_color(palette.faint)
                                                    .child(line)
                                                    .when_some(time, |meta, time| {
                                                        meta.tooltip(move |window, cx| {
                                                            Tooltip::new(time.clone()).build(window, cx)
                                                        })
                                                    }),
                                            )
                                        })
                                        .child(
                                            actions(last)
                                                .flex_none()
                                                .when(done, |row| row.child(copy))
                                                .when(done && last && self.asking.is_none(), |row| row.child(again))
                                                .when(entry.stopped, |row| {
                                                    row.child(
                                                        div()
                                                            .pl(px(4.))
                                                            .text_xs()
                                                            .text_color(palette.muted)
                                                            .child("Stopped"),
                                                    )
                                                }),
                                        ),
                                )
                                .children(list),
                        )
                    })
                    .into_any_element()
            }
        };
        // A message a rewind took back stays, faded (`.chat-message-rewound`).
        let element = if entry.stored.rewound {
            div()
                .w_full()
                .opacity(super::chat_rewind::REWOUND_OPACITY)
                .child(element)
                .into_any_element()
        } else {
            element
        };
        // A new chat's question and its answer's first line ride the start's clock (`launch_motion.rs`).
        let launched = match self.entries.iter().position(|other| other.id == entry.id) {
            Some(0) => Some(super::launch_motion::Part::Message),
            Some(1) => Some(super::launch_motion::Part::Working),
            _ => None,
        }
        .filter(|_| entry.arriving && self.launch_owns_arrival(cx));
        if let Some(part) = launched {
            self.launch_part(part, element)
        } else if entry.arriving {
            // chat-land for a question (12px), chat-rise for an answer starting (6px).
            let distance = if entry.role == Role::User { 12. } else { 6. };
            motion::arrive(div().w_full().child(element), ("arrive", entry.id), distance).into_any_element()
        } else {
            element
        }
    }
}

/// The mark and words an answer shows before its first text ([`motion::thinking`]).
fn render_thinking(id: usize, palette: Palette) -> impl IntoElement {
    div()
        .text_sm()
        .child(motion::thinking(format!("thinking-{id}"), id as u64, palette))
}

/// The arrow that brings the chat back to its end, while it is scrolled up (`render_jump`'s, for the list).
fn render_list_jump(
    list: ListState,
    phase: motion::Phase,
    palette: Palette,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let button = div()
        .id("jump-to-latest")
        .size(px(32.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.pop)
        .border_1()
        .border_color(palette.border)
        .shadow_md()
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        .active(|style| style.opacity(0.8))
        .on_click(cx.listener(move |_, _, _, cx| {
            list.set_follow_mode(FollowMode::Tail);
            cx.notify();
        }))
        .child(glyph(IconName::ArrowDown, palette.foreground));
    h_flex()
        .absolute()
        .bottom(px(12.))
        .left_0()
        .right_0()
        .justify_center()
        .child(phase.menu("chat-jump", 6., button))
}

/// `element`, placed in the transcript area as if the area ended at the floating dock's top, `room` from its foot.
pub(super) fn over_dock(room: Pixels, element: impl IntoElement) -> impl IntoElement {
    div().absolute().top_0().left_0().right_0().bottom(room).child(element)
}

/// Grey lines where a session's messages will be, while they load.
fn render_loading(palette: Palette) -> impl IntoElement {
    let line = |width: f32| div().h(px(12.)).w(relative(width)).rounded(px(4.)).bg(palette.selected);
    v_flex()
        .gap(px(12.))
        .child(
            h_flex()
                .justify_end()
                .child(div().h(px(40.)).w(px(260.)).rounded(px(16.)).bg(palette.selected)),
        )
        .child(line(0.92))
        .child(line(0.84))
        .child(line(0.6))
        .with_animation(
            "loading",
            Animation::new(motion::PULSE)
                .repeat()
                .with_max_fps(crate::motion::LOOP_FPS)
                .with_easing(pulsating_between(0.4, 0.9)),
            |column, delta| column.opacity(delta),
        )
}

/// Markdown as the Nexus app's `.prose` draws code (`chat-css.ts`): inline code in a small grey box (1px 5px, a 1px
/// line, radius 4, the strongest ink, 0.84em mono), and a block as a soft filled card (12px 14px, radius 6, the edge,
/// mono 12px). Code blocks and tables stay within the response width: long code wraps, and a table's columns shrink
/// to fit, then the table scrolls sideways. A code block's corner has its language and Copy (`code_block.rs`).
pub(crate) fn prose(view: TextView, palette: Palette) -> TextView {
    prose_running(view, palette, None)
}

/// [`prose`], with Run on a shell block that `run` runs (a Code session's answers).
pub(crate) fn prose_running(view: TextView, palette: Palette, run: Option<RunCommand>) -> TextView {
    let inline = StyleRefinement::default()
        .px(px(5.))
        .py(px(1.))
        .border_1()
        .border_color(palette.border)
        .rounded(px(4.))
        .bg(palette.sidebar)
        .text_color(palette.strong)
        .font_family("Geist Mono")
        .text_size(rems(0.84 * 14. / 16.));
    // A soft filled card with a 32 head for the language and actions (the design's code block):
    // the sidebar fill, radius 10, no outline, mono 13/20.
    let block = StyleRefinement::default()
        .px(px(12.))
        .pt(px(32.))
        .pb(px(12.))
        .rounded(px(10.))
        .border_0()
        .bg(palette.sidebar)
        .font_family("Geist Mono")
        .text_size(px(13.))
        .line_height(px(20.));
    // `overflow_x: scroll` is gpui-kit's adaptive table layout: columns fit their content, shrink to a floor, then
    // the table scrolls inside the response instead of growing past it.
    let mut table = StyleRefinement::default();
    table.overflow.x = Some(Overflow::Scroll);
    let style = TextViewStyle::default().code_block(block).table(table);
    // Resolve code blocks' percentage widths against the response column, not their content.
    view.w_full()
        .min_w_0()
        .max_w_full()
        .inline_code_style(inline)
        .style(style)
        .code_block_actions(move |block, _, _| code_block::actions(block, palette, run.as_ref()))
}

/// A 24px fade from the page's colour at the transcript's top, so text scrolls under the top bar softly rather than
/// being cut off. Only on an opaque window: a see-through veil (`frost.rs`) painted again over itself is darker than
/// the bar above it, a smudge under the bar. There, text meets the bar's edge plainly.
pub(super) fn top_fade(palette: Palette) -> impl IntoElement {
    let ground = crate::frost::Look::now(&palette).veil;
    div()
        .when(ground.a < 1., |fade| fade.invisible())
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(24.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(ground, 0.),
            linear_color_stop(ground.opacity(0.), 1.),
        ))
}

#[cfg(test)]
mod prose_tests {
    use gpui_kit::component::text::{TextView, TextViewState};
    use gpui_kit::{
        AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Styled, TestAppContext,
        Window, WindowAppearance, div, px,
    };

    use crate::palette::{Palette, ThemeChoice};

    struct Response {
        text: Entity<TextViewState>,
        width: f32,
    }

    impl Render for Response {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(ThemeChoice::Dark, WindowAppearance::Dark);
            div().w(px(self.width)).child(super::prose(
                TextView::new(&self.text)
                    .table_actions(|_, _, _| div().debug_selector(|| "table-corner".into()).size(px(1.))),
                palette,
            ))
        }
    }

    #[gpui_kit::test]
    fn code_response_fits_after_streaming_and_resizing(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let source = format!("```rust\nlet value = \"{}\";\n```", "long_code_".repeat(60));
        let (response, cx) = cx.add_window_view(|_, cx| Response {
            text: cx.new(|cx| TextViewState::markdown("```rust\nlet value =", cx)),
            width: 680.,
        });
        for width in [680., 240., 480.] {
            response.update(cx, |response, cx| {
                response.width = width;
                response.text.update(cx, |text, cx| text.set_text(&source, cx));
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let bounds = response.read_with(cx, |response, cx| response.text.read(cx).bounds());
            assert_eq!(bounds.size.width, px(width), "code must fit the response column");
            let corner = cx.debug_bounds("code-actions").expect("code block is drawn");
            assert!(
                corner.right() <= bounds.right(),
                "code block must stay within the response: {corner:?} in {bounds:?}"
            );
            assert!(bounds.size.height > px(40.), "long code must wrap within the column");
        }
    }

    #[gpui_kit::test]
    fn table_response_fits_after_streaming_and_resizing(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let row = |cell: &str| format!("| {cell} | {cell} | {cell} |\n");
        let source = format!(
            "{}| --- | --- | --- |\n{}",
            row("Case"),
            row(&"a_long_unbroken_cell_value_".repeat(4)).repeat(3)
        );
        let (response, cx) = cx.add_window_view(|_, cx| Response {
            text: cx.new(|cx| TextViewState::markdown("| Case |", cx)),
            width: 680.,
        });
        for width in [680., 240., 480.] {
            response.update(cx, |response, cx| {
                response.width = width;
                response.text.update(cx, |text, cx| text.set_text(&source, cx));
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let bounds = response.read_with(cx, |response, cx| response.text.read(cx).bounds());
            assert_eq!(bounds.size.width, px(width), "the table must not widen the response");
            let corner = cx.debug_bounds("table-corner").expect("table is drawn");
            assert!(
                corner.left() >= bounds.left() && corner.right() <= bounds.right(),
                "table must stay within the response: {corner:?} in {bounds:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatList, Entry, unchanged_prefix};

    /// A card's Open shows its page again when the Artifacts tab still holds it behind a closed Workbench.
    /// The Workbench opens on Artifacts and the page does not load again. A page the tab does not hold loads.
    #[gpui_kit::test]
    fn a_hidden_page_shows_again_without_a_reload(cx: &mut gpui_kit::TestAppContext) {
        use super::super::{Workspace, machine_source::MachineSource};
        use super::Tool;

        cx.update(gpui_kit::init);
        let (gateway, _gateway_events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let gateway: std::sync::Arc<dyn crate::backend::Backend> = std::sync::Arc::new(gateway);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::with_machine_source(gateway, events, arrivals, false, MachineSource::isolated(), window, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            let key = ("page".to_string(), 2);
            let chat = crate::gateway::ChatText {
                title: "Page".into(),
                markdown: "# Page".into(),
                path: "/artifacts/page".into(),
            };
            workspace.open_chat_text(chat, cx);
            workspace.page.as_mut().expect("the page").block = Some(key.clone());
            workspace.pick_tool(Tool::Artifacts, window, cx);
            assert!(workspace.page_card_open(&key), "on screen, the card reads Opened");
            workspace.side.open = false;
            assert!(!workspace.page_card_open(&key), "hidden, the card offers Open");

            let mut loads = 0;
            workspace.show_or_load_page(&key, window, cx, |_, _| loads += 1);
            assert_eq!(loads, 0, "the page does not load again");
            assert!(
                workspace.page_card_open(&key),
                "the Workbench is open on Artifacts again"
            );
            workspace.show_or_load_page(&("other".to_string(), 1), window, cx, |_, _| loads += 1);
            assert_eq!(loads, 1, "a page the tab does not hold loads");
        });
    }

    /// A long error under an answer wraps within the chat's column, with "Try again" after it in the
    /// column; a short one keeps "Try again" right after its text.
    #[gpui_kit::test]
    fn a_long_error_wraps_within_the_column(cx: &mut gpui_kit::TestAppContext) {
        use super::super::{Auth, View, Workspace, machine_source::MachineSource};
        use gpui_kit::px;

        cx.update(gpui_kit::init);
        let (gateway, _gateway_events) = crate::gateway::Gateway::start();
        let (_send, events) = futures::channel::mpsc::unbounded();
        let (_arrive, arrivals) = futures::channel::mpsc::unbounded();
        let gateway: std::sync::Arc<dyn crate::backend::Backend> = std::sync::Arc::new(gateway);
        let (workspace, cx) = cx.add_window_view(|window, cx| {
            Workspace::with_machine_source(gateway, events, arrivals, false, MachineSource::isolated(), window, cx)
        });
        let long = "The model provider refused the request because it is over its limit. ".repeat(8);
        for error in [long.as_str(), "Network error"] {
            workspace.update(cx, |workspace, cx| {
                workspace.auth = Auth::SignedIn;
                workspace.view = View::Chat;
                let mut answer = Entry::answer(2, "", false, cx);
                answer.error = Some(error.to_string().into());
                workspace.entries = vec![Entry::user(1, "Why?".into()), answer];
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
            // The row fills the centered column (`max_w(column_width())`), so its right edge is the column's.
            let column = cx.debug_bounds("chat-error-row").expect("the error row is drawn");
            let text = cx.debug_bounds("chat-error").expect("the error is drawn");
            let retry = cx.debug_bounds("chat-retry").expect("Try again is drawn");
            assert!(
                text.right() <= column.right(),
                "the error must stay in the column: {text:?} in {column:?}"
            );
            assert!(
                retry.right() <= column.right(),
                "Try again must stay in the column: {retry:?} in {column:?}"
            );
            if error == long {
                assert!(
                    text.size.height > px(30.),
                    "a long error wraps onto more lines: {text:?}"
                );
            } else {
                // "Network error" is about 90px wide: Try again follows it after the row's 10px gap, not at the
                // column's far edge.
                assert!(
                    retry.left() - text.left() < px(150.),
                    "Try again stays right after a short error: {retry:?} after {text:?} in {column:?}"
                );
            }
        }
    }

    fn chat(entries: &[Entry]) -> ChatList {
        let mut chat = ChatList::new();
        chat.sync(entries, false, &None);
        chat
    }

    fn users(count: usize) -> Vec<Entry> {
        (0..count).map(|id| Entry::user(id, format!("message {id}"))).collect()
    }

    /// Stop following, as a reader scrolling up does, so a later `sync` that follows again is seen.
    fn scrolled_up(chat: &ChatList) {
        chat.list.pause_following_tail();
        assert!(!chat.list.is_following_tail());
    }

    #[test]
    fn the_list_has_one_item_per_message_and_the_tail() {
        let entries = users(3);
        let chat = chat(&entries);
        assert_eq!(chat.list.item_count(), 4);
        assert_eq!(chat.shape.len(), 3);
    }

    #[test]
    fn an_append_measures_only_the_new_message_and_the_tail() {
        let mut entries = users(3);
        let mut chat = chat(&entries);
        let before = chat.shape.clone();
        entries.push(Entry::user(3, "next".into()));
        chat.sync(&entries, false, &None);
        assert_eq!(chat.list.item_count(), 5);
        assert_eq!(unchanged_prefix(&before, &chat.shape), 3);
    }

    #[test]
    fn a_growing_answer_measures_again_from_that_message_on() {
        let mut entries = users(3);
        let mut chat = chat(&entries);
        let before = chat.shape.clone();
        entries[1].streamed.push_str("more words");
        chat.sync(&entries, false, &None);
        // Same count, but the message that grew and everything after it are measured again.
        assert_eq!(chat.list.item_count(), 4);
        assert_eq!(unchanged_prefix(&before, &chat.shape), 1);
    }

    #[test]
    fn nothing_changed_means_nothing_is_measured() {
        let entries = users(3);
        let mut chat = chat(&entries);
        scrolled_up(&chat);
        chat.sync(&entries, false, &None);
        assert!(!chat.list.is_following_tail());
        assert_eq!(chat.list.item_count(), 4);
    }

    #[test]
    fn a_change_in_the_tail_alone_leaves_the_messages_and_the_reader_alone() {
        let entries = users(2);
        let mut chat = chat(&entries);
        scrolled_up(&chat);
        chat.sync(&entries, true, &None);
        assert_eq!(chat.list.item_count(), 3);
        assert!(!chat.list.is_following_tail());
    }

    #[test]
    fn a_removed_message_shrinks_the_list() {
        let mut entries = users(3);
        let mut chat = chat(&entries);
        entries.pop();
        chat.sync(&entries, false, &None);
        assert_eq!(chat.list.item_count(), 3);
    }

    #[test]
    fn a_session_opened_goes_to_the_end() {
        let entries = users(3);
        let mut chat = chat(&[]);
        scrolled_up(&chat);
        chat.sync(&entries, false, &None);
        assert!(chat.list.is_following_tail());
        // Opening an empty session has nothing to follow.
        let mut empty = ChatList::new();
        scrolled_up(&empty);
        empty.sync(&[], false, &None);
        assert!(!empty.list.is_following_tail());
    }

    #[test]
    fn a_question_sent_goes_to_the_end_wherever_the_reader_was() {
        let mut entries = users(3);
        let mut chat = chat(&entries);
        scrolled_up(&chat);
        let mut sent = Entry::user(3, "now".into());
        sent.arriving = true;
        entries.push(sent);
        chat.sync(&entries, false, &None);
        assert!(chat.list.is_following_tail());
    }

    #[test]
    fn a_message_that_loads_in_does_not_pull_the_reader_down() {
        let mut entries = users(3);
        let mut chat = chat(&entries);
        scrolled_up(&chat);
        // A new last message that is not arriving now (an older page, a refresh) is not a send.
        entries.push(Entry::user(3, "loaded".into()));
        chat.sync(&entries, false, &None);
        assert!(!chat.list.is_following_tail());
    }
}
