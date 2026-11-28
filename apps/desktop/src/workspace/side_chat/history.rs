//! The side thread's own events, drawn as the session transcript draws them (`code_view.rs`, `code_folds.rs`): fold
//! heads and rails, thoughts, tool rows, answers, messages and turn footers. Folds survive polling and tab switches.
use super::*;
use crate::palette::Palette;
use crate::workspace::code_folds::{self, Head, INDENT, Key, Place};
use crate::workspace::{answer_footer, code_view, glyph, tool_body};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use nexus_workbench::tool_rows;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct History {
    pub folds: HashMap<Key, bool>,
    pub all_folds: HashSet<isize>,
    pub(super) open: HashSet<String>,
    pub(super) all_output: HashSet<String>,
    /// The turn (its end's seq) and the message under the pointer: their actions show.
    pub(super) hovered_turn: Option<i64>,
    pub(super) hovered_message: Option<i64>,
    /// The queued row under the pointer, which opens to its whole message (`queued_row.rs`).
    pub(super) queued: crate::workspace::queued_row::Hovered,
    /// What Copy last copied: a message's seq, or a turn's as `-seq - 1`.
    pub(super) copied: Option<i64>,
}

/// The side transcript as a list that lays out only the rows near the screen (`code_view.rs`'s `transcript_list`): one
/// item per block, then the tail (the streamed answer, the now line, the welcome and the dock's room). It follows the
/// end while the end shows.
pub(super) struct HistoryList {
    pub(super) list: ListState,
    /// The blocks and rows the list was last measured for: only the rows that changed are measured again.
    blocks: Vec<Block>,
    places: Vec<Row>,
    tail: u64,
    palette: Option<Palette>,
}

/// How a block's row draws: its place in its run of work, what the person opened in it, and whether its answer's text
/// view is made yet.
#[derive(Clone, PartialEq)]
struct Row {
    place: Place,
    open: (bool, bool),
    text: bool,
}

impl HistoryList {
    pub(super) fn new() -> Self {
        let list = ListState::new(1, ListAlignment::Top, px(800.));
        list.set_follow_mode(FollowMode::Tail);
        Self {
            list,
            blocks: Vec::new(),
            places: Vec::new(),
            tail: 0,
            palette: None,
        }
    }

    /// Whether the reader is at the end: a new row then scrolls into view.
    pub(super) fn at_end(&self) -> bool {
        self.list.is_following_tail()
    }

    pub(super) fn to_end(&self) {
        self.list.set_follow_mode(FollowMode::Tail);
    }

    fn sync(&mut self, blocks: &[Block], places: Vec<Row>, tail: u64) {
        let (old_len, new_len) = (self.blocks.len(), blocks.len());
        let same = |old: usize, new: usize| self.blocks[old] == blocks[new] && self.places[old] == places[new];
        let prefix = (0..old_len.min(new_len)).take_while(|&ix| same(ix, ix)).count();
        let suffix = (1..=(old_len - prefix).min(new_len - prefix))
            .take_while(|&back| same(old_len - back, new_len - back))
            .count();
        let rows_changed = prefix + suffix < old_len.max(new_len);
        if rows_changed {
            let (old_end, new_end) = (old_len - suffix, new_len - suffix);
            let in_place = (old_end - prefix).min(new_end - prefix);
            self.list.remeasure_items(prefix..prefix + in_place);
            self.list
                .splice(prefix + in_place..old_end, new_end - prefix - in_place);
            self.blocks = blocks.to_vec();
            self.places = places;
        }
        // The tail is one item after the rows: it is measured again whenever anything changed.
        if rows_changed || tail != self.tail {
            self.list.remeasure_items(blocks.len()..blocks.len() + 1);
            self.tail = tail;
        }
    }
}

impl SideChat {
    /// Bring the history's list up to date with the blocks, their folds and the tail.
    pub(super) fn sync_history(&mut self, palette: Palette) {
        let places = code_folds::layout(&self.blocks, &self.history.folds, &self.history.all_folds, 0)
            .into_iter()
            .enumerate()
            .map(|(ix, place)| Row {
                place,
                open: self.row_open(ix),
                text: self.texts.contains_key(&ix),
            })
            .collect();
        let tail = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (
                self.streamed.as_ref().map(|(text, _)| text.as_str()),
                self.working_since.is_some(),
                self.is_empty() && !self.summary.status.busy(),
                f32::from(self.dock_height.room()).to_bits(),
            )
                .hash(&mut hasher);
            hasher.finish()
        };
        self.scroll.palette = Some(palette);
        self.scroll.sync(&self.blocks, places, tail);
    }

    /// One item of the history's list: a block's row, or the tail after the last one.
    pub(super) fn history_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self
            .scroll
            .palette
            .unwrap_or_else(|| Palette::for_choice(super::super::settings_view::saved_theme(), window.appearance()));
        let row = div().w_full().px(px(16.)).when(ix == 0, |row| row.pt(px(20.)));
        if let Some(Row { place, .. }) = self.scroll.places.get(ix).filter(|_| ix < self.blocks.len()) {
            let place = place.clone();
            return row.child(self.history_item(ix, &place, p, cx)).into_any_element();
        }
        let mut tail = Vec::new();
        if let Some((_, text)) = &self.streamed {
            tail.push(code_view::render_block(
                usize::MAX,
                &Block::Text(String::new()),
                Some(text.clone()),
                None,
                None,
                p,
                Arc::new(|href, _, _, cx| crate::external_link::open(href, cx)),
                None,
            ));
        }
        tail.extend(self.history_working(p));
        if self.is_empty() && !self.summary.status.busy() {
            tail.push(self.render_welcome(p, cx).into_any_element());
        }
        row.when(!tail.is_empty(), |row| {
            row.child(v_flex().w_full().pt(px(14.)).gap(px(14.)).children(tail))
        })
        .child(div().h(self.dock_height.room() + px(20.)).flex_none())
        .into_any_element()
    }

    /// What the person opened in row `ix` (a thought, a tool's input or its whole output): it changes the row's height.
    fn row_open(&self, ix: usize) -> (bool, bool) {
        match self.blocks[ix].inner() {
            Block::Thinking(..) => (self.history.open.contains(&format!("thought-{ix}")), false),
            Block::Tool { call, .. } => (
                self.history.open.contains(&call.id),
                self.history.all_output.contains(&call.id),
            ),
            _ => (false, false),
        }
    }

    fn history_head(
        &self,
        kind: &'static str,
        head: &Head,
        depth: u8,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (key, open) = (head.key, head.open);
        let anchor = key.anchor();
        code_folds::head_row(kind, head, depth, p)
            .debug_selector(move || format!("{kind}-{anchor}"))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.history.folds.insert(key, !open);
                cx.notify();
            }))
            .into_any_element()
    }

    /// What block `ix` draws above its own row: its card's header, "Show N earlier steps", its fold's row.
    pub(super) fn history_lead(&self, place: &Place, p: Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut lead = Vec::new();
        if let Some(head) = &place.header {
            lead.push(self.history_head("side-work", head, 0, p, cx));
        }
        if let Some((key, steps)) = place.earlier {
            lead.push(
                code_folds::earlier_row("side-earlier", key, steps, p)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.history.all_folds.insert(key);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if let Some(head) = &place.fold {
            lead.push(self.history_head("side-fold", head, 1, p, cx));
        }
        lead
    }

    fn copy(&mut self, key: i64, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.history.copied = Some(key);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |this, cx| {
                if this.history.copied == Some(key) {
                    this.history.copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// One row of the side transcript, in the session's column: its lead, then the block unless its fold is closed.
    pub(super) fn history_item(&self, ix: usize, place: &Place, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let block = &self.blocks[ix];
        if let Block::Tool { name, .. } = block.inner()
            && code_view::hidden_tool(name)
        {
            return div().into_any_element();
        }
        if matches!(block.inner(), Block::Queued { .. }) {
            return div().into_any_element();
        }
        let lead = self.history_lead(place, p, cx);
        let lead = (!lead.is_empty()).then(|| v_flex().w_full().pt(px(if ix == 0 { 0. } else { 14. })).children(lead));
        if place.hidden {
            return div().w_full().children(lead).into_any_element();
        }
        let first = ix == 0 && lead.is_none();
        let rail = place.depth > 0;
        let agent_depth = block.path().len();
        let top = px(match (lead.is_some(), first) {
            (true, _) => 4.,
            (false, true) => 0.,
            (false, false) if rail => 0.,
            (false, false) if agent_depth > 0 => 4.,
            (false, false) => 14.,
        });
        let column = if rail {
            let depth = agent_depth + usize::from(place.depth) - 1;
            v_flex()
                .w_full()
                .ml(px(INDENT * depth as f32 + 6.5))
                .pl(px(15.))
                .border_l_1()
                .border_color(p.border)
        } else {
            v_flex().w_full().pl(px(INDENT * agent_depth as f32))
        };
        let top = match block.inner() {
            Block::Turn { .. } if !first => px(4.),
            Block::User { .. } if !first => px(14.),
            _ => top,
        };
        div()
            .id(("side-row", ix))
            .w_full()
            .children(lead)
            .child(column.pt(top).child(self.history_block(ix, block.inner(), p, cx)))
            .into_any_element()
    }

    pub(super) fn history_block(&self, ix: usize, block: &Block, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        match block {
            Block::Text(_) => {
                let link: crate::workspace::link_menu::LinkHandler =
                    Arc::new(|href, _, _, cx| crate::external_link::open(href, cx));
                code_view::render_block(ix, block, self.texts.get(&ix).cloned(), None, None, p, link, None)
            }
            Block::User {
                seq, at, text, from, ..
            } => self.history_user(*seq, *at, text, from.is_some(), p, cx),
            Block::Thinking(text, ms) => {
                let key = format!("thought-{ix}");
                let open = self.history.open.contains(&key);
                v_flex()
                    .w_full()
                    .child(
                        code_folds::thought_head("side-thought", ix, *ms, open, p).on_click(cx.listener(
                            move |this, _, _, cx| {
                                if !this.history.open.remove(&key) {
                                    this.history.open.insert(key.clone());
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .when(open, |row| row.child(code_folds::thought_body(text, p)))
                    .into_any_element()
            }
            Block::Tool {
                name,
                summary,
                call,
                error: Some(error),
                ..
            } => self.history_failed_tool(ix, name, summary, &call.id, error, p, cx),
            Block::Tool {
                name, summary, call, ..
            } => self.history_tool(ix, name, summary, call, p, cx),
            Block::Turn {
                seq,
                status,
                duration_ms,
                cost_usd,
                tokens,
                model,
                ..
            } => self.history_turn(
                ix,
                *seq,
                *status,
                *duration_ms,
                *cost_usd,
                *tokens,
                model.as_deref(),
                p,
                cx,
            ),
            Block::Agent(agent) => h_flex()
                .w_full()
                .min_h(px(28.))
                .gap(px(8.))
                .text_size(px(13.))
                .child(code_view::agent_mark(ix, agent.status, p))
                .child(
                    div()
                        .flex_none()
                        .text_color(p.foreground)
                        .child(if agent.title.is_empty() {
                            "Sub-agent".to_string()
                        } else {
                            agent.title.clone()
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(p.faint)
                        .child(agent.aside().unwrap_or_default().to_string()),
                )
                .into_any_element(),
            Block::AgentEnd(end) => div()
                .text_size(px(13.))
                .text_color(p.muted)
                .children(end.outcome.clone())
                .into_any_element(),
            Block::Plan(todos) => v_flex()
                .gap(px(4.))
                .text_size(px(13.))
                .children(todos.iter().map(|todo| {
                    h_flex()
                        .gap(px(8.))
                        .child(glyph(
                            if todo.status == code::TodoStatus::Completed {
                                Lucide::SquareCheck
                            } else {
                                Lucide::Square
                            },
                            p.faint,
                        ))
                        .child(todo.content.clone())
                }))
                .into_any_element(),
            Block::Page { title, page_id, .. } => div()
                .p(px(12.))
                .rounded(px(12.))
                .bg(p.raised)
                .child(title.clone().unwrap_or_else(|| page_id.clone()))
                .into_any_element(),
            block => code_view::render_block(
                ix,
                block,
                None,
                None,
                None,
                p,
                Arc::new(|href, _, _, cx| crate::external_link::open(href, cx)),
                None,
            ),
        }
    }

    /// A message, as the session's: the bubble at the right, its time under it, and Copy on hover.
    fn history_user(
        &self,
        seq: i64,
        at: i64,
        text: &str,
        linked: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let copied = self.history.copied == Some(seq);
        let shown = copied || self.history.hovered_message == Some(seq);
        let copy = {
            let text = text.to_string();
            answer_footer::hover_button(
                ("side-user-copy", seq as u64),
                if copied { Lucide::Check } else { Lucide::Copy },
                if copied { "Copied" } else { "Copy your message" },
                p,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.copy(seq, text.clone(), cx)))
        };
        let time = (at > 0).then(|| {
            let exact: SharedString = code::message_time_exact(at).into();
            div()
                .id(("side-user-time", seq as u64))
                .text_size(px(12.))
                .text_color(p.faint)
                .whitespace_nowrap()
                .tooltip(move |window, cx| Tooltip::new(exact.clone()).build(window, cx))
                .child(code::message_time(at, code::now_ms()))
        });
        v_flex()
            .id(("side-user-message", seq as u64))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.history.hovered_message = Some(seq);
                } else if this.history.hovered_message == Some(seq) {
                    this.history.hovered_message = None;
                }
                cx.notify();
            }))
            .w_full()
            .items_end()
            .gap(px(4.))
            .pt(px(10.))
            .when(linked, |column| {
                column.child(
                    div()
                        .text_color(p.faint)
                        .text_size(px(12.))
                        .child("From another session"),
                )
            })
            .child(code_view::user_prompt(text.to_string(), vec![], None, p, None))
            .child(
                h_flex()
                    .h(px(24.))
                    .gap(px(2.))
                    .items_center()
                    .children(time)
                    .child(div().when(!shown, |slot| slot.opacity(0.)).child(copy)),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn history_failed_tool(
        &self,
        ix: usize,
        name: &str,
        summary: &str,
        id: &str,
        error: &str,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.history.open.contains(id);
        let row = code_view::tool_row(
            ("side-tool", ix),
            glyph(Lucide::CircleX, p.destructive),
            name,
            summary,
            false,
            p,
        )
        .debug_selector(move || format!("side-tool-{ix}"))
        .child(
            div()
                .flex_none()
                .text_size(px(13.))
                .text_color(p.destructive)
                .child("Failed"),
        );
        let id = id.to_string();
        let row = if error.is_empty() {
            row
        } else {
            row.cursor_pointer()
                .child(crate::workspace::disclosure("disclosure", open, p.faint))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.history.open.remove(&id) {
                        this.history.open.insert(id.clone());
                    }
                    cx.notify();
                }))
        };
        v_flex()
            .w_full()
            .gap(px(4.))
            .child(row)
            .when(open && !error.is_empty(), |column| {
                column.child(
                    div()
                        .w_full()
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(8.))
                        .bg(p.raised)
                        .font_family("Geist Mono")
                        .text_size(px(11.5))
                        .line_height(px(18.))
                        .text_color(p.muted)
                        .child(error.to_string()),
                )
            })
            .into_any_element()
    }

    fn history_tool(
        &self,
        ix: usize,
        name: &str,
        summary: &str,
        call: &crate::code::ToolCall,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = call.id.clone();
        let open = self.history.open.contains(&id);
        let running = call.output.is_none() && self.summary.status.busy();
        let group: SharedString = format!("side-tool-row-{ix}").into();
        let row = code_view::tool_row(
            ("side-tool", ix),
            code_view::tool_mark(name, p),
            name,
            summary,
            running,
            p,
        )
        .debug_selector(move || format!("side-tool-{ix}"))
        .group(group.clone())
        .cursor_pointer()
        .child(
            div()
                .flex_none()
                .when(!open, |slot| {
                    slot.opacity(0.).group_hover(group.clone(), |style| style.opacity(1.))
                })
                .child(crate::workspace::disclosure("disclosure", open, p.faint)),
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            if !this.history.open.remove(&id) {
                this.history.open.insert(id.clone());
            }
            cx.notify();
        }));
        v_flex()
            .w_full()
            .gap(px(4.))
            .child(row)
            .when(open, |row| row.child(self.history_tool_output(ix, name, call, p, cx)))
            .into_any_element()
    }

    /// A turn's end, as the session's: the maker's mark and how it ended, then Copy on hover.
    #[allow(clippy::too_many_arguments)]
    fn history_turn(
        &self,
        ix: usize,
        seq: i64,
        status: nexus_workbench::event::TurnStatus,
        duration_ms: u64,
        cost_usd: Option<f64>,
        tokens: u64,
        model: Option<&str>,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use nexus_workbench::event::TurnStatus;
        let copy_key = -seq - 1;
        let copied = self.history.copied == Some(copy_key);
        // The latest answer's actions stay, as the session's do; they carry Bring back.
        let bring_back = (self.bring_back_turn() == Some(ix)).then(|| self.bring_back_action(p, cx));
        let shown = copied || bring_back.is_some() || self.history.hovered_turn == Some(seq);
        let copy = code_view::turn_answer(&self.blocks, ix)
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                answer_footer::hover_button(
                    ("side-turn-copy", seq as u64),
                    if copied { Lucide::Check } else { Lucide::Copy },
                    if copied { "Copied" } else { "Copy" },
                    p,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.copy(copy_key, text.clone(), cx)))
            });
        let ended = match status {
            TurnStatus::Interrupted => Some("Stopped"),
            TurnStatus::Error => Some("Failed"),
            TurnStatus::Completed => None,
        };
        let name = model.map(code::model_label);
        let about = [
            Some(answer_footer::words(name.as_deref(), cost_usd, None)).filter(|words| !words.is_empty()),
            (duration_ms > 0).then(|| format!("Worked for {}", code::duration(duration_ms))),
            (tokens > 0).then(|| format!("{} tokens", code::compact(tokens))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        h_flex()
            .id(("side-turn-row", ix))
            .debug_selector(move || format!("side-turn-{ix}"))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.history.hovered_turn = Some(seq);
                } else if this.history.hovered_turn == Some(seq) {
                    this.history.hovered_turn = None;
                }
                cx.notify();
            }))
            .w_full()
            .h(px(28.))
            .gap(px(8.))
            .child(answer_footer::line(
                ("side-turn", ix),
                ended.map(str::to_string).unwrap_or_default(),
                (!about.is_empty()).then(|| about.into()),
                status == TurnStatus::Error,
                shown,
                p,
            ))
            .child(div().flex_1())
            .child(
                h_flex()
                    .flex_none()
                    .gap(px(2.))
                    .when(!shown, |row| row.opacity(0.))
                    .children(copy)
                    .children(bring_back),
            )
            .into_any_element()
    }

    fn history_tool_output(
        &self,
        ix: usize,
        name: &str,
        call: &crate::code::ToolCall,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let all = self.history.all_output.contains(&call.id);
        let mut body = v_flex()
            .w_full()
            .pl(px(22.))
            .gap(px(6.))
            .child(tool_body::tool_input(name, call, p));
        if let Some(output) = &call.output {
            let preview = tool_rows::preview_output(&output.text);
            let shown = if all || preview.more == 0 {
                output.text.clone()
            } else {
                format!(
                    "{}\n{}\n{}",
                    preview.head,
                    tool_rows::more_lines_label(preview.more),
                    preview.tail
                )
            };
            body = body.child(tool_body::pre(shown, p));
            if !all && preview.more > 0 {
                let id = call.id.clone();
                body = body.child(
                    div()
                        .id(("side-output-all", ix))
                        .debug_selector(move || format!("side-output-all-{ix}"))
                        .text_size(px(12.5))
                        .text_color(p.muted)
                        .cursor_pointer()
                        .child(tool_rows::show_all_label(preview.total))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.history.all_output.insert(id.clone());
                            cx.notify();
                        })),
                );
            }
            if output.truncated {
                body = body.child(div().text_size(px(12.)).text_color(p.faint).child("Cut at 64 KiB."));
            }
        } else {
            body = body.child(div().text_size(px(12.)).text_color(p.faint).child("No result yet."));
        }
        body.into_any_element()
    }

    /// The now line, as the session's: what the side agent does, for how long, and "Esc to stop".
    pub(super) fn history_working(&self, p: Palette) -> Option<AnyElement> {
        let since = self.working_since?;
        let waiting = code::pending_approval(&self.blocks).is_some() || self.secret_id.is_some();
        let (now, verb, target) = code_view::now_line(
            None,
            waiting,
            code_view::now_doing(&self.blocks),
            code_view::thought_summary(&self.blocks),
        );
        Some(code_view::working_line(
            now,
            verb,
            target,
            since,
            !waiting,
            code_view::turn_seed("side-chat", &self.blocks),
            p,
        ))
    }
}
