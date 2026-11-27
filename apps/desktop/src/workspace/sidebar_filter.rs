//! The sidebar's session filter, as the Nexus app's (`desktop/src/workbench/sidebar.tsx`, `sidebar-logic.ts`'s
//! `matchesQuery`, `message-search.tsx`): `/` (with no text field focused) or Ctrl+Shift+F (Cmd+Shift+F on macOS) opens
//! a field above the lists. Typing narrows chats, Code sessions and projects by title, project or branch as you type,
//! every word having to match. From 3 letters on an "In messages" section adds the sessions whose prompts or answers
//! hold the words: Code sessions from the local store (`wb_search`), chats from Gateway's search (`GET /v1/search`).
//! The Projects magnifier opens a project-only input in that header, without searching chats or messages.
//! Down and Up walk the rows, Enter opens one, Esc empties the field and then closes it.

use crate::workspace::overlay::OverlaySlot;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::sidebar::{ROW, ROW_TEXT};
use super::{Workspace, glyph};
use crate::code::{CodeProject, CodeSession, CodeStore};
use crate::gateway::SearchHit;
use crate::i18n::tr;
use crate::model::SessionSummary;
use crate::palette::Palette;

/// Message search starts at this many typed characters (the index matches 3+ letters at a time).
const MIN_MESSAGE_QUERY: usize = 3;
/// Typing pauses this long before the messages are searched (`PAUSE_MS`).
const PAUSE: Duration = Duration::from_millis(150);
/// Message hits shown per source.
const HITS: usize = 8;
/// Gateway search answers carry this bit in their number, so the palette's own answers are told apart.
pub(super) const SEQ_BIT: u64 = 1 << 62;

/// A row of "In messages".
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MessageHit {
    pub id: String,
    pub chat: bool,
    pub title: String,
    /// The line around the match, the match between `\u{2}` and `\u{3}`.
    pub snippet: String,
    /// The Code session's event that matched: the session opens at it. None for a chat.
    pub seq: Option<i64>,
}

/// What the filter row under the cursor opens.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Open {
    Chat(String),
    Code(String),
    /// A Code session at one of its events: a message hit.
    CodeAt(String, i64),
}

impl MessageHit {
    /// What the hit opens: its chat, or its Code session at the message.
    fn open(&self) -> Open {
        match self.seq {
            _ if self.chat => Open::Chat(self.id.clone()),
            Some(seq) => Open::CodeAt(self.id.clone(), seq),
            None => Open::Code(self.id.clone()),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Scope {
    #[default]
    Sidebar,
    Projects,
}

#[derive(Default)]
pub(super) struct SidebarFilter {
    pub open: bool,
    scope: Scope,
    pub scroll: ScrollHandle,
    saved_scroll: Option<Point<Pixels>>,
    pub project_trigger: Option<FocusHandle>,
    input: Option<Entity<InputState>>,
    query: String,
    /// The row Down/Up reached, by [`Open`] target and whether it is a message hit.
    cursor: Option<(Open, bool)>,
    hits: Vec<MessageHit>,
    seq: u64,
    pause: Option<Task<()>>,
    _subscription: Option<Subscription>,
    /// The Code sessions picked for a bulk action ([`super::sidebar_select`]).
    pub picked: super::sidebar_select::Picked,
}

impl SidebarFilter {
    /// The words typed, which every row has to match: none shows everything.
    pub fn active(&self) -> bool {
        self.open && !self.query.trim().is_empty()
    }
}

/// The words of a query, folded for comparing.
fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether every word of `query` is inside one of `fields`, in any case ("nexus fix" finds the fix sessions of the
/// nexus project). Nothing typed matches all.
pub(super) fn matches(query: &str, fields: &[&str]) -> bool {
    let fields: Vec<String> = fields.iter().map(|field| field.to_lowercase()).collect();
    words(query)
        .iter()
        .all(|word| fields.iter().any(|field| field.contains(word.as_str())))
}

/// Message search runs from this many characters typed.
pub(super) fn searches_messages(query: &str) -> bool {
    query.trim().chars().count() >= MIN_MESSAGE_QUERY
}

/// A snippet as plain and matched runs: the core puts each match between `\u{2}` and `\u{3}`.
pub(super) fn snippet_parts(snippet: &str) -> Vec<(String, bool)> {
    let mut parts = Vec::new();
    let (mut hit, mut text) = (false, String::new());
    for ch in snippet.chars() {
        match ch {
            '\u{2}' | '\u{3}' => {
                if !text.is_empty() {
                    parts.push((std::mem::take(&mut text), hit));
                }
                hit = ch == '\u{2}';
            }
            '\n' | '\r' | '\t' => text.push(' '),
            _ => text.push(ch),
        }
    }
    if !text.is_empty() {
        parts.push((text, hit));
    }
    parts
}

/// A Code session's filter fields: its title, its project's name and branch, its own branch.
fn session_matches(query: &str, session: &CodeSession, project: &CodeProject) -> bool {
    matches(
        query,
        &[
            &session.title,
            &project.name,
            project.branch.as_deref().unwrap_or(""),
            session.branch.as_deref().unwrap_or(""),
        ],
    )
}

/// The sidebar rows the filter keeps, in the order they are drawn.
pub(super) struct Kept<'a> {
    pub chats: Vec<&'a SessionSummary>,
    pub projects: Vec<(&'a CodeProject, Vec<&'a CodeSession>)>,
}

impl Workspace {
    pub(super) fn project_filter_open(&self) -> bool {
        self.sidebar_filter.open && self.sidebar_filter.scope == Scope::Projects
    }

    /// The Projects head no longer has a search button (the window head's search finds projects); the tests still drive the scope.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn toggle_project_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_filter_open() {
            self.close_filter(window, cx);
        } else {
            self.open_filter_scope(Scope::Projects, window, cx);
        }
    }

    pub(super) fn filtering(&self) -> bool {
        self.sidebar_filter.active()
    }

    /// Chats and projects narrowed by the query. A project stays when its own name or branch matches (with all its
    /// sessions that match), or when a session does; one with no sessions stays when its name matches.
    pub(super) fn filter_kept(&self) -> Kept<'_> {
        let query = self.sidebar_filter.query.as_str();
        let chats = self
            .sessions
            .iter()
            .filter(|chat| self.sidebar_filter.scope == Scope::Sidebar && matches(query, &[&chat.title]))
            .collect();
        let projects = self
            .projects
            .iter()
            .filter_map(|project| {
                let sessions: Vec<&CodeSession> = project
                    .sessions
                    .iter()
                    .filter(|session| session_matches(query, session, project))
                    .collect();
                let own = project.sessions.is_empty()
                    && matches(query, &[&project.name, project.branch.as_deref().unwrap_or("")]);
                (!sessions.is_empty() || own).then_some((project, sessions))
            })
            .collect();
        Kept { chats, projects }
    }

    /// Every row in the order drawn: chats, project sessions, then message hits.
    fn filter_targets(&self) -> Vec<(Open, bool)> {
        let kept = self.filter_kept();
        let mut targets: Vec<(Open, bool)> = kept
            .chats
            .iter()
            .map(|chat| (Open::Chat(chat.id.clone()), false))
            .collect();
        targets.extend(
            kept.projects
                .iter()
                .flat_map(|(_, sessions)| super::session_tree::tree(sessions))
                .map(|(session, _)| (Open::Code(session.id.clone()), false)),
        );
        targets.extend(self.filter_hits().iter().map(|hit| (hit.open(), true)));
        targets
    }

    /// The message hits to show: the answers of the current words,
    pub(super) fn filter_hits(&self) -> Vec<&MessageHit> {
        if self.sidebar_filter.scope == Scope::Projects || !searches_messages(&self.sidebar_filter.query) {
            return Vec::new();
        }
        self.sidebar_filter.hits.iter().collect()
    }

    pub(super) fn filter_cursor(&self) -> Option<&(Open, bool)> {
        self.sidebar_filter.cursor.as_ref()
    }

    /// Opens the field with focus in it (its text selected when it was open already).
    pub(super) fn open_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_filter_scope(Scope::Sidebar, window, cx);
    }

    fn open_filter_scope(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sidebar_filter.open {
            self.sidebar_filter.saved_scroll = Some(self.sidebar_filter.scroll.offset());
        }
        if self.sidebar_filter.scope != scope {
            self.sidebar_filter.query.clear();
            self.sidebar_filter.hits.clear();
            self.sidebar_filter.cursor = None;
            self.sidebar_filter.pause = None;
            self.sidebar_filter.seq += 1;
            if let Some(input) = self.sidebar_filter.input.clone() {
                input.update(cx, |state, cx| state.set_value("", window, cx));
            }
        }
        self.sidebar_filter.scope = scope;
        if scope == Scope::Projects && self.sidebar_filter.project_trigger.is_none() {
            self.sidebar_filter.project_trigger = Some(cx.focus_handle());
        }
        if !self.sidebar_open {
            self.toggle_sidebar(cx);
        }
        let input = match self.sidebar_filter.input.clone() {
            Some(input) => input,
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("Search sessions")));
                self.sidebar_filter._subscription =
                    Some(
                        cx.subscribe_in(&input, window, |this, state, event: &InputEvent, _, cx| {
                            if let InputEvent::Change = event {
                                let query = state.read(cx).value().to_string();
                                if query != this.sidebar_filter.query {
                                    this.filter_typed(query, cx);
                                }
                            }
                        }),
                    );
                self.sidebar_filter.input = Some(input.clone());
                input
            }
        };
        self.sidebar_filter.open = true;
        input.update(cx, |state, cx| {
            state.set_placeholder(
                if scope == Scope::Projects {
                    tr("Search projects…")
                } else {
                    tr("Search sessions")
                },
                window,
                cx,
            );
            state.focus(window, cx);
        });
        cx.notify();
    }

    /// Clears search and restores focus to its trigger, or the composer for sidebar-wide search.
    pub(super) fn close_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_filter.open = false;
        self.sidebar_filter.query.clear();
        self.sidebar_filter.hits.clear();
        self.sidebar_filter.cursor = None;
        self.sidebar_filter.pause = None;
        self.sidebar_filter.seq += 1;
        if let Some(input) = self.sidebar_filter.input.clone() {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        if let Some(offset) = self.sidebar_filter.saved_scroll.take() {
            self.sidebar_filter.scroll.set_offset(offset);
        }
        if self.sidebar_filter.scope == Scope::Projects {
            if let Some(focus) = &self.sidebar_filter.project_trigger {
                focus.focus(window, cx);
            }
        } else {
            self.focus_composer(window, cx);
        }
        cx.notify();
    }

    pub(super) fn clear_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = self.sidebar_filter.input.clone() {
            input.update(cx, |state, cx| {
                state.set_value("", window, cx);
                state.focus(window, cx);
            });
        }
        self.filter_typed(String::new(), cx);
    }

    /// The query changed: rows narrow at once, messages are searched once typing pauses.
    fn filter_typed(&mut self, query: String, cx: &mut Context<Self>) {
        self.sidebar_filter.query = query;
        self.sidebar_filter.cursor = None;
        self.sidebar_filter.seq += 1;
        let seq = self.sidebar_filter.seq;
        let typed = self.sidebar_filter.query.trim().to_string();
        if self.account_gateway().is_none()
            || self.sidebar_filter.scope == Scope::Projects
            || !searches_messages(&typed)
        {
            self.sidebar_filter.hits.clear();
            self.sidebar_filter.pause = None;
        } else {
            let reader = self.code.as_ref().map(CodeStore::reader);
            self.sidebar_filter.pause = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(PAUSE).await;
                // The full-text search scans the whole history: run it on the background executor.
                let query = typed.clone();
                let code = cx
                    .background_executor()
                    .spawn(async move { reader.and_then(|reader| reader.search(&query, HITS).ok()) })
                    .await
                    .unwrap_or_default();
                let _ = this.update(cx, |this, cx| {
                    if this.sidebar_filter.seq != seq {
                        return;
                    }
                    // Code sessions are in the local store, chats are Gateway's: they land one after the other.
                    let hits: Vec<MessageHit> = code
                        .into_iter()
                        .filter_map(|(id, seq, snippet)| {
                            let title = this.session_by_id(&id)?.title.clone();
                            Some(MessageHit {
                                id,
                                chat: false,
                                title,
                                snippet,
                                seq: Some(seq),
                            })
                        })
                        .collect();
                    this.sidebar_filter.hits = hits;
                    if let Some(account) = this.account_gateway() {
                        account.search(seq | SEQ_BIT, typed);
                    }
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    /// Gateway's answer for the filter: its chats join "In messages".
    pub(super) fn filter_searched(
        &mut self,
        seq: u64,
        results: Result<Vec<SearchHit>, String>,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_filter.scope != Scope::Sidebar || seq & !SEQ_BIT != self.sidebar_filter.seq {
            return;
        }
        let Ok(hits) = results else {
            return;
        };
        let titled: Vec<String> = self.filter_kept().chats.iter().map(|chat| chat.id.clone()).collect();
        let chats: Vec<MessageHit> = hits
            .into_iter()
            .filter(|hit| hit.entity == "session" && !titled.contains(&hit.id))
            .take(HITS)
            .map(|hit| MessageHit {
                id: hit.id,
                chat: true,
                title: hit.title,
                snippet: hit.subtitle.unwrap_or_default(),
                seq: None,
            })
            .collect();
        self.sidebar_filter.hits.retain(|hit| !hit.chat);
        self.sidebar_filter.hits.extend(chats);
        cx.notify();
    }

    /// Opens what a filter row names; the filter stays, so the next one is a Down away.
    pub(super) fn filter_open_row(&mut self, target: &Open, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            Open::Chat(id) => {
                let chat = self.sessions.iter().find(|chat| &chat.id == id).cloned().or_else(|| {
                    let hit = self.sidebar_filter.hits.iter().find(|hit| hit.chat && &hit.id == id)?;
                    Some(SessionSummary {
                        id: id.clone(),
                        title: hit.title.clone(),
                        day: None,
                        at_ms: None,
                    })
                });
                if let Some(chat) = chat {
                    self.open(&chat, window, cx);
                }
            }
            Open::Code(id) => self.open_code(id, window, cx),
            Open::CodeAt(id, seq) => self.open_code_at(id, *seq, window, cx),
        }
    }

    /// Keys while the filter is open and focus is in the sidebar's field: Down/Up walk rows, Enter opens, Esc clears
    /// and then closes. Whether the key was the filter's.
    pub(super) fn filter_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match key {
            "escape" => {
                if self.sidebar_filter.query.is_empty() {
                    self.close_filter(window, cx);
                } else {
                    self.clear_filter(window, cx);
                }
                true
            }
            "down" | "up" => {
                let targets = self.filter_targets();
                if targets.is_empty() {
                    return true;
                }
                let at = self
                    .sidebar_filter
                    .cursor
                    .as_ref()
                    .and_then(|cursor| targets.iter().position(|target| target == cursor));
                let next = match (key, at) {
                    ("down", None) => 0,
                    ("down", Some(at)) => (at + 1) % targets.len(),
                    (_, None) | (_, Some(0)) => targets.len() - 1,
                    (_, Some(at)) => at - 1,
                };
                self.sidebar_filter.cursor = Some(targets[next].clone());
                cx.notify();
                true
            }
            "enter" => {
                let targets = self.filter_targets();
                let target = self.sidebar_filter.cursor.clone().or_else(|| targets.first().cloned());
                if let Some((open, _)) = target {
                    self.filter_open_row(&open, window, cx);
                }
                true
            }
            _ => false,
        }
    }

    /// Whether a plain key is for the filter: `/` and Ctrl+Shift+F open it, the rest only while its field has focus.
    pub(super) fn on_filter_keystroke(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if keystroke.key == "escape" && !keystroke.modifiers.modified() && self.picked_escape(cx) {
            return true;
        }
        if !matches!(self.view, super::View::Settings(_)) {
            let modifiers = &keystroke.modifiers;
            let chord = if cfg!(target_os = "macos") {
                modifiers.platform
            } else {
                modifiers.control
            };
            if keystroke.key == "f" && chord && modifiers.shift && !modifiers.alt {
                self.open_filter(window, cx);
                return true;
            }
            let typing = window.context_stack().iter().any(|context| context.contains("Input"));
            if keystroke.key == "/" && !modifiers.modified() && !typing && !self.overlay.palette().is_some() {
                self.open_filter(window, cx);
                return true;
            }
        }
        let focused = self
            .sidebar_filter
            .input
            .as_ref()
            .is_some_and(|input| input.focus_handle(cx).is_focused(window));
        self.sidebar_filter.open
            && focused
            && !keystroke.modifiers.modified()
            && self.filter_key(keystroke.key.as_str(), window, cx)
    }

    /// Marks the row Down/Up reached with the hover tone.
    pub(super) fn filter_cursor_mark(&self, row: AnyElement, target: &Open, palette: Palette) -> AnyElement {
        if self.filter_cursor() == Some(&(target.clone(), false)) {
            div().rounded(px(8.)).bg(palette.hover).child(row).into_any_element()
        } else {
            row
        }
    }

    /// The field: a search glyph and the input on a filled 28px bar, with no border.
    pub(super) fn render_filter_field(&self, palette: Palette) -> Option<AnyElement> {
        let input = self
            .sidebar_filter
            .input
            .as_ref()
            .filter(|_| self.sidebar_filter.open && self.sidebar_filter.scope == Scope::Sidebar)?;
        Some(
            h_flex()
                .h(px(28.))
                .mx(px(8.))
                .mb(px(4.))
                .px(px(8.))
                .gap(px(6.))
                .rounded(px(8.))
                .bg(palette.hover)
                .text_xs()
                .child(glyph(Lucide::Search, palette.muted).size(px(13.)))
                .child(div().flex_1().min_w(px(0.)).child(Input::new(input).appearance(false)))
                .into_any_element(),
        )
    }

    /// The Projects title's slot becomes the input; the outer close button never moves.
    pub(super) fn render_project_filter_input(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some(input) = self
            .sidebar_filter
            .input
            .as_ref()
            .filter(|_| self.project_filter_open())
        else {
            return div().flex_1().min_w(px(0.)).child(tr("Projects")).into_any_element();
        };
        h_flex()
            .flex_1()
            .min_w(px(0.))
            .h(px(28.))
            .px(px(6.))
            .gap(px(4.))
            .rounded(px(6.))
            .bg(palette.hover)
            .border_1()
            .border_color(palette.muted)
            .child(glyph(Lucide::Search, palette.muted).size(px(13.)))
            .child(div().flex_1().min_w(px(0.)).child(Input::new(input).appearance(false)))
            .when(!self.sidebar_filter.query.is_empty(), |field| {
                field.child(
                    super::sidebar::small_button("clear-project-search", Lucide::X, palette)
                        .focusable()
                        .tab_index(0)
                        .tooltip(|window, cx| Tooltip::new(tr("Clear search")).build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.clear_filter(window, cx)))
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                cx.stop_propagation();
                                this.clear_filter(window, cx);
                            }
                        })),
                )
            })
            .into_any_element()
    }

    /// "In messages": the sessions whose messages hold the words, each with a line around the match.
    pub(super) fn render_message_hits(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let hits = self.filter_hits();
        if hits.is_empty() {
            return None;
        }
        let cursor = self.filter_cursor().cloned();
        let rows: Vec<AnyElement> = hits
            .into_iter()
            .map(|hit| {
                let open = hit.open();
                let under = cursor.as_ref() == Some(&(open.clone(), true));
                let snippet = h_flex()
                    .overflow_hidden()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .children(snippet_parts(&hit.snippet).into_iter().map(|(text, hit)| {
                        div()
                            .when(hit, |part| {
                                part.text_color(palette.foreground).font_weight(FontWeight::MEDIUM)
                            })
                            .child(text)
                    }));
                v_flex()
                    .id(SharedString::from(format!("message-hit-{}", hit.id)))
                    .h(px(ROW + 12.))
                    .px(px(8.))
                    .justify_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_color(palette.muted)
                    .when(under, |row| row.bg(palette.hover).text_color(palette.foreground))
                    .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
                    .child(div().truncate().text_size(px(ROW_TEXT)).child(hit.title.clone()))
                    .child(snippet)
                    .on_click(cx.listener(move |this, _, window, cx| this.filter_open_row(&open, window, cx)))
                    .into_any_element()
            })
            .collect();
        Some(
            v_flex()
                .child(
                    h_flex()
                        .h(px(28.))
                        .mt(px(16.))
                        .pl(px(8.))
                        .text_xs()
                        .text_color(palette.faint)
                        .child(tr("In messages")),
                )
                .children(rows)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "sidebar_filter_tests.rs"]
mod tests;
