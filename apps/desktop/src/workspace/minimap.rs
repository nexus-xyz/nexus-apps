//! The scroll minimap of a long chat or Code session, as the Nexus app's (`ui/.../nexus-ui/chat/minimap.tsx`): a thin rail
//! at the area's right edge, one 6px dot per message you sent, brighter while
//! on screen, under a pill that spans what is on screen. Answers keep their place on the rail without a dot. Hovering the rail previews the nearest message's
//! first line, and a click jumps to it. A press and drag takes the chat along: the list follows the message under the
//! pointer until the button is released. The wheel over the rail scrolls the chat. A Code session's rows are its messages
//! and its answers, each answer a turn of several items (`code_view.rs`'s `minimap_rows`).
//!
//! A conversation longer than the rail holds at [`PITCH`] gets a window instead: the stops around what is on screen,
//! with how many rows lie past each end, and a thin bar beside them for where the screen sits among the loaded rows.
//! A drag holds the window still, so the stops do not move under the pointer, and slides it while held past an end.
//! The window is held as a count from the end: an older page landing above leaves it where it was.
//!
//! The Nexus app puts a tick where its message starts, in proportion to the thread's height. GPUI's list measures only
//! the messages near the screen and keeps their offsets to itself, so here the ticks are spaced evenly, one place per
//! message. The rail reads the list as it was last laid out; a canvas after the list asks for one more frame when that
//! changed, so the rail never lags a frame behind a scroll or a resize.

use gpui_kit::prelude::*;
use gpui_kit::*;

use std::cell::Cell;
use std::rc::Rc;

use super::Workspace;
use crate::model::Role;
use crate::palette::Palette;

/// The rail's width: its hit area. The ticks hang from its right edge.
const RAIL: f32 = 24.;
/// Under this width the area has no rail (the right panel open at 1440 leaves 704).
const MIN_AREA: f32 = 720.;
/// The track's drawn width at the rail's right end; dots and the in-view pill sit in it.
const TRACK: f32 = 12.;
/// The rail's least pitch: a 2px tick and 3px of air. Closer than this, ticks read as one hatched bar.
const TICK_GAP: f32 = 5.;
/// A windowed rail's pitch: one stop every 10px, the closest a pointer still picks one from its neighbours.
const PITCH: f32 = 10.;
/// How often a drag held past a windowed rail's end slides it.
const SLIDE_EVERY: std::time::Duration = std::time::Duration::from_millis(40);
/// A held slide moves one row a step, and one more for each this many px the pointer is past the end.
const SLIDE_RAMP: f32 = 24.;
/// The longest first line the preview shows, in characters.
pub(super) const PREVIEW: usize = 90;

/// One tick: where it sits on the rail, and the messages it stands for (`first` to `last`, as indexes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Tick {
    pub y: f32,
    pub first: usize,
    pub last: usize,
    /// One of its messages is the person's own.
    pub user: bool,
}

/// One row of a list's minimap: a message, or an answer that spans several items. `item` is the list item it
/// starts at, where a click on its tick scrolls to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Row {
    pub item: usize,
    /// It is the person's own message.
    pub user: bool,
}

/// What a list keeps of its rail between frames: the stop under the pointer and a drag under way. It lives with the
/// list (`ChatList`, `CodeView`), so a drag ends with the list it scrolls.
#[derive(Default)]
pub(super) struct MinimapState {
    pub hover: Option<usize>,
    pub drag: Option<Drag>,
}

/// Where the workspace keeps a list's [`MinimapState`]; None once that list is gone.
pub(super) type Slot = fn(&mut Workspace) -> Option<&mut MinimapState>;

/// A list's minimap: the list, its rows in order, and its state.
pub(super) struct Minimap<'a> {
    pub id: &'static str,
    pub list: ListState,
    pub rows: Vec<Row>,
    /// The item after the last row's: the tail (the loading lines, the working line), which no tick stands for.
    pub end: usize,
    /// The centered column's width with its gutters (`Workspace::column_width`).
    pub column: f32,
    pub state: &'a MinimapState,
    pub slot: Slot,
}

/// A press and drag on a rail.
pub(super) struct Drag {
    /// A windowed rail's window while the drag holds it, as how many rows its first stop is from the end: the chat
    /// scrolling under the pointer must not move the stops the pointer picks from, and an older page lands above. A
    /// row added at the end while the drag holds (a new turn) moves the window by that row.
    pub from_end: usize,
    /// The rows and the window's stops at the rail's last frame, for the slide to stay inside. The render (`&self`)
    /// keeps them current.
    pub rows: Cell<usize>,
    pub size: Cell<usize>,
    /// Set by each frame that draws the rail, and taken by each slide step: a step that finds it unset ends the drag,
    /// whose rail has stopped drawing (the area narrowed, the view changed) and whose release no listener would see.
    pub drawn: Cell<bool>,
    /// How far past the window's top (below zero) or bottom (above) the pointer is, in px; zero over it.
    pub over: f32,
    /// The timer that slides the window while the pointer is past an end. Dropping it stops it.
    pub slide: Option<Task<()>>,
}

impl Drag {
    /// The window's first row over `count` rows in `size` stops.
    pub(super) fn start(&self, count: usize, size: usize) -> usize {
        count.saturating_sub(self.from_end).min(count.saturating_sub(size))
    }
}

/// Where the rail goes in the chat's area, and what of the chat shows, as of the list's last layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Rail {
    /// The rail's left edge and top, in the chat's area.
    left: f32,
    top: f32,
    height: f32,
    /// The rail's left edge and top in the window, to place the pointer on it.
    window_left: f32,
    window_top: f32,
    /// The indexes of the messages on screen.
    shown: Option<(usize, usize)>,
}

impl Rail {
    /// Whether `position`, in the window, is over the rail's hit area.
    fn contains(&self, position: Point<Pixels>) -> bool {
        let (x, y) = (f32::from(position.x), f32::from(position.y));
        (self.window_left..=self.window_left + RAIL).contains(&x)
            && (self.window_top..=self.window_top + self.height).contains(&y)
    }
}

/// The rail for a list of `rows` (each the list item it starts at) beside a `column` px wide column with its gutters
/// (`chat_column.rs`), or None when it has nothing to scroll or the gutter has no room for it. `end` is the item after
/// the last row's: the tail, which no tick stands for.
fn rail(list: &ListState, items: &[usize], end: usize, column: f32) -> Option<Rail> {
    if items.len() < 2 || list.max_offset_for_scrollbar().y <= px(0.) {
        return None;
    }
    let viewport = list.viewport_bounds();
    let (left, height) = place(
        f32::from(viewport.size.width),
        f32::from(viewport.size.height),
        items.len(),
        column,
    )?;
    let top = (f32::from(viewport.size.height) - height) / 2.;
    Some(Rail {
        left,
        top,
        height,
        window_left: f32::from(viewport.left()) + left,
        window_top: f32::from(viewport.top()) + top,
        shown: shown(list, items, end),
    })
}

/// The rail's left edge and height in an area `width` by `height`, or None without room. The rail hugs the area's
/// right edge (12 in), and hides when the area is under [`MIN_AREA`] wide or leaves no
/// gutter beside the centered column.
fn place(width: f32, height: f32, count: usize, column: f32) -> Option<(f32, f32)> {
    let edge = width.min((width + column) / 2.);
    let gutter = width - edge;
    let rail = rail_height(count, height);
    if width < MIN_AREA || gutter < RAIL + 12. || rail <= 0. {
        return None;
    }
    Some(((width - 12. - RAIL).round(), rail))
}

/// The rows on screen: the one the list's top is in, through the last that is not below the screen.
fn shown(list: &ListState, items: &[usize], end: usize) -> Option<(usize, usize)> {
    let first = list.logical_scroll_top().item_ix;
    if first >= end {
        return None;
    }
    let mut last = first;
    while last + 1 < end && list.item_is_below_viewport(last + 1) == Some(false) {
        last += 1;
    }
    shown_rows(items, first, last)
}

/// The rows that items `first` to `last` of the list belong to. A row runs from its item to the next row's.
pub(super) fn shown_rows(items: &[usize], first: usize, last: usize) -> Option<(usize, usize)> {
    let end = items.partition_point(|&item| item <= last);
    if end == 0 {
        return None;
    }
    Some((items.partition_point(|&item| item <= first).saturating_sub(1), end - 1))
}

/// The rail's height for `count` messages in `room` px: 10px a message, at least 96px, and never more than the room
/// less 24px at each end. Zero when the room is too small to be worth a rail.
pub(super) fn rail_height(count: usize, room: f32) -> f32 {
    let most = room - 48.;
    if most < 96. {
        return 0.;
    }
    most.min((count as f32 * 10.).max(96.))
}

/// The ticks for messages (`true` for the person's own) on a rail `height` px tall, evenly spaced. When even that would
/// put them closer than `TICK_GAP`, the messages that land on one place share one tick.
pub(super) fn layout_ticks(users: &[bool], height: f32) -> Vec<Tick> {
    if users.is_empty() || height <= 0. {
        return Vec::new();
    }
    let pitch = if users.len() > 1 {
        height / (users.len() - 1) as f32
    } else {
        0.
    };
    let tick = |i: usize, y: f32| Tick {
        y,
        first: i,
        last: i,
        user: users[i],
    };
    if users.len() == 1 || pitch >= TICK_GAP {
        return (0..users.len()).map(|i| tick(i, i as f32 * pitch)).collect();
    }
    let places = (height / TICK_GAP).floor();
    let mut ticks: Vec<Tick> = Vec::new();
    for (i, &user) in users.iter().enumerate() {
        let place = (i as f32 * pitch / TICK_GAP).round().min(places);
        match ticks.last_mut() {
            Some(last) if (last.y / TICK_GAP).round() >= place => {
                last.last = i;
                last.user |= user;
            }
            _ => ticks.push(tick(i, place * TICK_GAP)),
        }
    }
    ticks
}

/// How many stops a rail `height` px tall holds at [`PITCH`].
pub(super) fn window_size(height: f32) -> usize {
    (height.max(0.) / PITCH).floor() as usize + 1
}

/// The first row of a `size`-stop window over `count` rows, centred on the rows on screen (the end when none is).
pub(super) fn window_start(count: usize, size: usize, shown: Option<(usize, usize)>) -> usize {
    let max = count.saturating_sub(size);
    let centre = shown.map_or(count, |(first, last)| (first + last) / 2);
    centre.saturating_sub(size / 2).min(max)
}

/// The stops for rows `start` to `start + size`, [`PITCH`] apart, numbered as in `users`.
pub(super) fn window_ticks(users: &[bool], start: usize, size: usize) -> Vec<Tick> {
    (start..(start + size).min(users.len()))
        .enumerate()
        .map(|(i, row)| Tick {
            y: i as f32 * PITCH,
            first: row,
            last: row,
            user: users[row],
        })
        .collect()
}

/// How far `y` (from the rail's top) is past a window whose last stop sits at `bottom`: below zero above the top,
/// above zero below the bottom, zero over it. Half a pitch of slack at each end.
pub(super) fn overshoot(y: f32, bottom: f32) -> f32 {
    let slack = PITCH / 2.;
    if y < -slack {
        y + slack
    } else if y > bottom + slack {
        y - bottom - slack
    } else {
        0.
    }
}

/// Where a slide moves a held window, as rows from the end (see [`Drag`]): up or down by one row and one more each
/// [`SLIDE_RAMP`] px past the end, between the last window (`size` from the end) and the first (`rows`).
pub(super) fn slide(from_end: usize, rows: usize, size: usize, over: f32) -> usize {
    let step = 1 + (over.abs() / SLIDE_RAMP) as usize;
    if over < 0. {
        (from_end + step).min(rows.max(size))
    } else if over > 0. {
        from_end.saturating_sub(step).max(size.min(rows))
    } else {
        from_end
    }
}

/// Ends the drag in `state`, if there is one: a release that did not slide jumps to the stop `at`, as a click does,
/// and one off the rail drops the preview. Says whether there was a drag to end.
fn release(state: &mut MinimapState, at: Option<usize>, on_rail: bool, go: &dyn Fn(usize)) -> bool {
    let Some(drag) = state.drag.take() else {
        return false;
    };
    if drag.over == 0.
        && let Some(at) = at
    {
        go(at);
    }
    if !on_rail {
        state.hover = None;
    }
    true
}

/// The message a tick jumps to: its first question, else its first message.
pub(super) fn tick_target(tick: &Tick, users: &[bool]) -> usize {
    (tick.first..=tick.last)
        .find(|&i| users.get(i).copied().unwrap_or(false))
        .unwrap_or(tick.first)
}

/// Whether a tick stands for a message on screen.
fn tick_shown(tick: &Tick, shown: Option<(usize, usize)>) -> bool {
    shown.is_some_and(|(first, last)| tick.last >= first && tick.first <= last)
}

/// The nearest tick to `y` on the rail. Ticks are in order, so it is a binary search.
pub(super) fn nearest_tick(ticks: &[Tick], y: f32) -> Option<usize> {
    if ticks.is_empty() {
        return None;
    }
    let at = ticks.partition_point(|tick| tick.y < y).min(ticks.len() - 1);
    if at > 0 && (ticks[at - 1].y - y).abs() <= (ticks[at].y - y).abs() {
        return Some(at - 1);
    }
    Some(at)
}

/// The first line of a message as the preview shows it: the first line with text, Markdown marks and code fences
/// dropped, cut at `max` characters with an ellipsis (`firstLine` in the Nexus app's `minimap-layout.ts`).
pub(super) fn first_line(content: &str, max: usize) -> String {
    let mut fenced = false;
    for raw in content.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || trimmed == "$$" {
            continue;
        }
        let line = strip_marks(strip_lead(trimmed));
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() || line.chars().all(|c| matches!(c, '-' | '=' | '|' | ':' | ' ')) {
            continue;
        }
        if line.chars().count() > max {
            let cut: String = line.chars().take(max - 1).collect();
            return format!("{}…", cut.trim_end());
        }
        return line;
    }
    String::new()
}

/// A line without its block mark: a heading's hashes, a quote's `>`, a list's bullet or number, a task's box.
fn strip_lead(line: &str) -> &str {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(char::is_whitespace) {
        return line[hashes..].trim_start();
    }
    if let Some(rest) = line.strip_prefix('>') {
        return rest.trim_start();
    }
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            let rest = rest.trim_start();
            for task in ["[ ] ", "[x] ", "[X] "] {
                if let Some(text) = rest.strip_prefix(task) {
                    return text.trim_start();
                }
            }
            return rest;
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return rest.trim_start();
        }
    }
    line
}

/// Inline Markdown as plain text: a link or an image as its text, a citation (`[3]`) dropped, and no `*_`~` marks.
fn strip_marks(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let image = chars[i] == '!' && chars.get(i + 1) == Some(&'[');
        let open = if image { i + 1 } else { i };
        if chars[open] == '['
            && let Some(close) = chars[open + 1..].iter().position(|&c| c == ']').map(|at| open + 1 + at)
        {
            let text: String = chars[open + 1..close].iter().collect();
            let target = (chars.get(close + 1) == Some(&'('))
                .then(|| {
                    chars[close + 2..]
                        .iter()
                        .position(|&c| c == ')')
                        .map(|at| close + 2 + at)
                })
                .flatten();
            if let Some(end) = target {
                out.push_str(&text);
                i = end + 1;
                continue;
            }
            if !image && !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
                i = close + 1;
                continue;
            }
        }
        if !matches!(chars[i], '*' | '_' | '`' | '~') {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

impl Workspace {
    /// The rail beside the open chat: one row per message.
    pub(super) fn render_chat_minimap(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .entries
            .iter()
            .enumerate()
            .map(|(item, entry)| Row {
                item,
                user: entry.role == Role::User,
            })
            .collect();
        let map = Minimap {
            id: "chat-minimap",
            list: self.chat_list.list.clone(),
            rows,
            end: self.entries.len(),
            column: self.column_width(),
            state: &self.chat_list.minimap,
            slot: chat_minimap,
        };
        self.render_minimap(
            map,
            &|row| {
                let entry = &self.entries[row];
                let text = match entry.role {
                    Role::User => first_line(&entry.text, PREVIEW),
                    Role::Assistant => first_line(&entry.streamed, PREVIEW),
                };
                let text = if !text.is_empty() {
                    text
                } else if !entry.attachments.is_empty() {
                    "Attachment".into()
                } else {
                    "No text".into()
                };
                (if entry.role == Role::User { "You" } else { "Answer" }, text)
            },
            palette,
            cx,
        )
    }

    /// A list's rail (see the module's comment), and the canvas that keeps it in step with the list. `preview` gives
    /// a row's label and first line, for the tick under the pointer.
    pub(super) fn render_minimap(
        &self,
        map: Minimap<'_>,
        preview: &dyn Fn(usize) -> (&'static str, String),
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items: Vec<usize> = map.rows.iter().map(|row| row.item).collect();
        let (end, column) = (map.end, map.column);
        let drawn = rail(&map.list, &items, end, column);
        // A drag whose rail is not drawn (the area narrowed under it) has no listener of its own, and its release goes
        // unseen: this canvas, drawn every frame, ends it at the first move without the button.
        let orphan = (drawn.is_none() && map.state.drag.is_some()).then(|| (cx.entity().downgrade(), map.slot));
        let watch = {
            let list = map.list.clone();
            canvas(
                move |_, window, _| {
                    // The list laid out after this was rendered: draw again if the rail would move.
                    if rail(&list, &items, end, column) != drawn {
                        window.refresh();
                    }
                },
                move |_, _, window, _| {
                    let Some((workspace, slot)) = orphan else {
                        return;
                    };
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && event.pressed_button != Some(MouseButton::Left) {
                            end_drag(&workspace, slot, cx);
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        };
        let element = drawn.map(|rail| render_rail(map, rail, preview, palette, cx));
        // Pinned to the area's corner: with no inset, it would sit after the list, below the area's foot.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_0()
            .child(watch)
            .children(element)
            .into_any_element()
    }
}

impl Workspace {
    /// Slides the dragged window of the list at `slot` while the pointer is held past an end, one timer a drag. It
    /// stops when the drag ends, comes back over the rail, or reaches the last window. At the first window it keeps
    /// asking for frames: the rail's paint scrolls to the top stop, and the Code view reads an older page when its
    /// list nears the top (`code_view.rs`, `OLDER_MARGIN`).
    fn slide_minimap(&mut self, slot: Slot, cx: &mut Context<Self>) {
        let Some(drag) = slot(self).and_then(|state| state.drag.as_mut()) else {
            return;
        };
        if drag.slide.is_some() {
            return;
        }
        drag.slide = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(SLIDE_EVERY).await;
                let going = this
                    .update(cx, |this, cx| {
                        let Some(state) = slot(this) else {
                            return false;
                        };
                        let Some(drag) = state.drag.as_mut() else {
                            return false;
                        };
                        if !drag.drawn.replace(false) {
                            state.drag = None;
                            state.hover = None;
                            cx.notify();
                            return false;
                        }
                        let next = slide(drag.from_end, drag.rows.get(), drag.size.get(), drag.over);
                        if drag.over == 0. || (drag.over > 0. && next == drag.from_end) {
                            // Dropping its own task: it stops at this await's end.
                            drag.slide = None;
                            return false;
                        }
                        drag.from_end = next;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !going {
                    break;
                }
            }
        }));
    }
}

/// Ends the drag of the list at `slot` without a jump, its preview with it.
fn end_drag(workspace: &WeakEntity<Workspace>, slot: Slot, cx: &mut App) {
    let _ = workspace.update(cx, |this, cx| {
        if let Some(state) = slot(this)
            && state.drag.take().is_some()
        {
            state.hover = None;
            cx.notify();
        }
    });
}

/// The open chat's minimap state.
fn chat_minimap(this: &mut Workspace) -> Option<&mut MinimapState> {
    Some(&mut this.chat_list.minimap)
}

fn render_rail(
    map: Minimap<'_>,
    rail: Rail,
    preview: &dyn Fn(usize) -> (&'static str, String),
    palette: Palette,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let Minimap {
        id,
        list,
        rows,
        state,
        slot,
        ..
    } = map;
    let users: Vec<bool> = rows.iter().map(|row| row.user).collect();
    let count = users.len();
    let size = window_size(rail.height);
    let windowed = count > size;
    let drag = state.drag.as_ref();
    if let Some(drag) = drag {
        drag.rows.set(count);
        drag.size.set(size);
        drag.drawn.set(true);
    }
    let start = match drag {
        Some(drag) if windowed => drag.start(count, size),
        _ if windowed => window_start(count, size, rail.shown),
        _ => 0,
    };
    let ticks = if windowed {
        window_ticks(&users, start, size)
    } else {
        layout_ticks(&users, rail.height)
    };
    let bottom = ticks.last().map_or(0., |tick| tick.y);
    let last = ticks.len().saturating_sub(1);
    let dragging = drag.is_some();
    let held_over = drag.map_or(0., |drag| drag.over);
    let hover = state.hover.filter(|&at| at < ticks.len());
    let tip = hover.map(|at| {
        let (who, text) = preview(tick_target(&ticks[at], &users));
        (ticks[at].y, who, text)
    });

    let pointer = {
        let ticks = ticks.clone();
        move |position: Point<Pixels>| nearest_tick(&ticks, f32::from(position.y) - rail.window_top)
    };
    // Past a windowed rail's end; zero for one that shows every row, which has nothing to slide to.
    let past = move |position: Point<Pixels>| {
        if windowed {
            overshoot(f32::from(position.y) - rail.window_top, bottom)
        } else {
            0.
        }
    };
    // Scrolls the list to the message tick `at` stands for.
    let go = {
        let list = list.clone();
        let ticks = ticks.clone();
        let users = users.clone();
        let items: Vec<usize> = rows.iter().map(|row| row.item).collect();
        Rc::new(move |at: usize| {
            if let Some(tick) = ticks.get(at) {
                list.scroll_to(ListOffset {
                    item_ix: items[tick_target(tick, &users)],
                    offset_in_item: px(0.),
                });
            }
        })
    };
    // While the left button drags this rail, the window's moves and its release are the rail's, wherever the pointer
    // goes: a drag that slides off the narrow rail keeps scrolling, and a release anywhere ends it. Capture runs them
    // before any element's own listener. Held past a windowed rail's end, each frame scrolls to the stop at that end
    // as the timer slides the window.
    let follow = dragging.then(|| {
        let workspace = cx.entity().downgrade();
        let (pointer, go) = (pointer.clone(), go.clone());
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                if held_over != 0. {
                    go(if held_over < 0. { 0 } else { last });
                }
                let (moving, at_move, go_move) = (workspace.clone(), pointer.clone(), go.clone());
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let held = event.pressed_button == Some(MouseButton::Left);
                    let over = past(event.position);
                    let at = if over < 0. {
                        Some(0)
                    } else if over > 0. {
                        Some(last)
                    } else {
                        at_move(event.position)
                    };
                    let _ = moving.update(cx, |this, cx| {
                        let Some(state) = slot(this) else {
                            return;
                        };
                        let Some(drag) = state.drag.as_mut() else {
                            return;
                        };
                        if !held {
                            // The release went elsewhere: the drag is over, where it got to.
                            state.drag = None;
                            state.hover = None;
                            cx.notify();
                            return;
                        }
                        drag.over = over;
                        // Every move scrolls, not only a move to another tick: the stop under a still pointer can
                        // stand for another message once an older page lands.
                        state.hover = at;
                        if let Some(at) = at {
                            go_move(at);
                        }
                        if over != 0. {
                            this.slide_minimap(slot, cx);
                        }
                        cx.notify();
                    });
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                        return;
                    }
                    let (at, on_rail) = (pointer(event.position), rail.contains(event.position));
                    let _ = workspace.update(cx, |this, cx| {
                        if let Some(state) = slot(this)
                            && release(state, at, on_rail, &*go)
                        {
                            cx.notify();
                        }
                    });
                });
            },
        )
        .absolute()
        .size_0()
    });
    let (hover_at, press_at, tap_at, tap_go) = (pointer.clone(), pointer.clone(), pointer, go);

    div()
        .id(id)
        .absolute()
        .left(px(rail.left))
        .top(px(rail.top))
        .w(px(RAIL))
        .h(px(rail.height))
        .cursor_pointer()
        .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
            let at = hover_at(event.position);
            if let Some(state) = slot(this)
                && state.drag.is_none()
                && state.hover != at
            {
                state.hover = at;
                cx.notify();
            }
        }))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            // A drag keeps its preview off the rail: the release clears it.
            if !hovered
                && let Some(state) = slot(this)
                && state.drag.is_none()
                && state.hover.take().is_some()
            {
                cx.notify();
            }
        }))
        .on_scroll(cx.listener(move |_, event: &ScrollEvent, window, cx| {
            let delta = event.delta.pixel_delta(window.line_height());
            if delta.y > px(0.) {
                list.pause_following_tail();
            }
            list.scroll_by(-delta.y);
            cx.notify();
        }))
        // The press starts the gesture; the release jumps (`release`). Stopping it keeps GPUI Kit's window selection
        // from starting a text selection under the rail, and `prevent_default` keeps the focus where it is (the
        // composer keeps the keyboard). The Code view's capture-phase cite listener still sees it.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                if let Some(state) = slot(this) {
                    state.drag = Some(Drag {
                        from_end: count - start,
                        rows: Cell::new(count),
                        size: Cell::new(size),
                        drawn: Cell::new(true),
                        over: 0.,
                        slide: None,
                    });
                    state.hover = press_at(event.position);
                }
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
            }),
        )
        // A tap can release before the frame that adds `follow`: the rail ends that drag. Only a drag this list
        // started: a text selection released over the rail is not a click on it.
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                if let Some(state) = slot(this)
                    && release(state, tap_at(event.position), true, &*tap_go)
                {
                    cx.notify();
                }
            }),
        )
        .children(follow)
        // A windowed rail: how many rows lie past each end, and where the screen sits among the loaded rows.
        .when(windowed && start > 0, |rail| {
            rail.child(end_count(format!("↑ {start}"), -20., palette))
        })
        .when(windowed && start + ticks.len() < count, |rail| {
            rail.child(end_count(
                format!("↓ {}", count - start - ticks.len()),
                bottom + 8.,
                palette,
            ))
        })
        .children(windowed.then(|| position_bar(rail.shown, count, bottom, palette)))
        // What is on screen, as a pill behind its dots.
        .children({
            let on: Vec<f32> = ticks
                .iter()
                .filter(|tick| tick_shown(tick, rail.shown))
                .map(|tick| tick.y)
                .collect();
            let first = on.iter().copied().reduce(f32::min);
            let last = on.iter().copied().reduce(f32::max);
            first.zip(last).map(|(first, last)| {
                let top = (first - 12.).max(-6.);
                div()
                    .absolute()
                    .right_0()
                    .top(px(top))
                    .w(px(TRACK))
                    .h(px((last + 12.).min(rail.height + 6.) - top))
                    .rounded_full()
                    .bg(palette.hover)
            })
        })
        // One 6 dot per message you sent: brighter on screen, the strongest ink under the pointer.
        .children(ticks.iter().enumerate().filter(|(_, tick)| tick.user).map(|(i, tick)| {
            let color = if hover == Some(i) {
                palette.strong
            } else if tick_shown(tick, rail.shown) {
                palette.muted
            } else {
                palette.faint.opacity(0.45)
            };
            div()
                .absolute()
                .right(px(3.))
                .top(px(tick.y - 3.))
                .size(px(6.))
                .rounded_full()
                .bg(color)
        }))
        // An answer under the pointer has no dot: a ring marks the stop the preview and a click stand for.
        .children(hover.filter(|&at| !ticks[at].user).map(|at| {
            div()
                .absolute()
                .right(px(3.))
                .top(px(ticks[at].y - 3.))
                .size(px(6.))
                .rounded_full()
                .border_1()
                .border_color(palette.strong)
        }))
        .when_some(tip, |rail, (y, who, text)| {
            // The first line of the row under the pointer, left of the rail, centred on its tick.
            rail.child(
                h_flex_tip()
                    .absolute()
                    .right(px(RAIL + 12.))
                    .top(px(y - 15.))
                    .h(px(30.))
                    .max_w(px(320.))
                    .px(px(10.))
                    .rounded(px(12.))
                    .bg(palette.menu)
                    .shadow(palette.pop_shadow(palette.pop_ring()))
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(who),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .truncate()
                            .text_color(palette.foreground)
                            .child(text),
                    ),
            )
        })
        .into_any_element()
}

/// A windowed rail's bar beside the stops, `span` px tall: where the rows on screen sit among all `count` rows.
fn position_bar(shown: Option<(usize, usize)>, count: usize, span: f32, palette: Palette) -> Div {
    let end = count.saturating_sub(1);
    let (first, last) = shown.unwrap_or((end, end));
    let span = span.max(1.);
    let top = first as f32 / count.max(1) as f32 * span;
    let height = ((last + 1) as f32 / count.max(1) as f32 * span - top).max(4.);
    div()
        .absolute()
        .right(px(TRACK + 3.))
        .top_0()
        .w(px(2.))
        .h(px(span))
        .rounded_full()
        .bg(palette.faint.opacity(0.25))
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(top))
                .w(px(2.))
                .h(px(height))
                .rounded_full()
                .bg(palette.muted),
        )
}

/// A windowed rail's count of the rows past one end, right-aligned over the track at `top`.
fn end_count(text: String, top: f32, palette: Palette) -> Div {
    div()
        .absolute()
        .right_0()
        .top(px(top))
        .text_size(px(10.))
        .line_height(px(12.))
        .whitespace_nowrap()
        .text_color(palette.faint)
        .child(text)
}

/// The preview's row: its parts on one baseline, 8px apart, never wrapped.
fn h_flex_tip() -> Div {
    div().flex().flex_row().items_center().gap(px(8.)).whitespace_nowrap()
}

#[cfg(test)]
mod tests {
    use super::{
        Drag, MinimapState, PITCH, first_line, layout_ticks, nearest_tick, overshoot, place, rail_height, release,
        shown_rows, slide, tick_target, window_size, window_start, window_ticks,
    };
    use std::cell::{Cell, RefCell};

    #[test]
    fn ticks_spread_evenly_down_the_rail() {
        let ticks = layout_ticks(&[true, false, true], 100.);
        let ys: Vec<f32> = ticks.iter().map(|tick| tick.y).collect();
        assert_eq!(ys, [0., 50., 100.]);
        assert!(ticks[0].user && !ticks[1].user);
    }

    #[test]
    fn crowded_messages_share_ticks_at_least_a_gap_apart() {
        let users: Vec<bool> = (0..200).map(|i| i % 2 == 0).collect();
        let ticks = layout_ticks(&users, 100.);
        assert!(ticks.len() <= 21);
        assert!(ticks.windows(2).all(|pair| pair[1].y - pair[0].y >= 5.));
        // Every message has a tick, in order, and none twice.
        assert_eq!(ticks[0].first, 0);
        assert_eq!(ticks.last().unwrap().last, 199);
        assert!(ticks.windows(2).all(|pair| pair[1].first == pair[0].last + 1));
        assert!(ticks.iter().all(|tick| tick.user));
    }

    #[test]
    fn a_shared_tick_jumps_to_its_first_question() {
        let users = [false, false, true, false];
        let ticks = layout_ticks(&users, 4.);
        assert_eq!(ticks.len(), 1);
        assert_eq!(tick_target(&ticks[0], &users), 2);
        let answers = [false, false];
        assert_eq!(tick_target(&layout_ticks(&answers, 4.)[0], &answers), 0);
    }

    #[test]
    fn the_pointer_picks_the_nearest_tick() {
        let ticks = layout_ticks(&[true, false, true], 100.);
        assert_eq!(nearest_tick(&ticks, -10.), Some(0));
        assert_eq!(nearest_tick(&ticks, 24.), Some(0));
        assert_eq!(nearest_tick(&ticks, 26.), Some(1));
        assert_eq!(nearest_tick(&ticks, 400.), Some(2));
        assert_eq!(nearest_tick(&[], 4.), None);
    }

    #[test]
    fn the_rail_grows_with_the_chat_inside_the_room() {
        assert_eq!(rail_height(3, 600.), 96.);
        assert_eq!(rail_height(30, 600.), 300.);
        assert_eq!(rail_height(300, 600.), 552.);
        assert_eq!(rail_height(30, 120.), 0.);
    }

    #[test]
    fn the_rail_needs_a_gutter() {
        // A window as wide as the column has no gutter.
        assert_eq!(place(728., 800., 20, 680.), None);
        // A wide one puts the rail at the area's right edge, 12 in.
        let (left, _) = place(1184., 800., 20, 728.).unwrap();
        assert_eq!(left, 1184. - 12. - 24.);
        // The right panel open leaves 704: no rail, as chat.
        assert_eq!(place(704., 800., 20, 640.), None);
    }

    #[test]
    fn a_row_shows_while_any_of_its_items_does() {
        // A chat: one item per row.
        assert_eq!(shown_rows(&[0, 1, 2, 3], 1, 2), Some((1, 2)));
        // A Code session: rows start at items 2, 5 and 9; items 6 to 7 are the second row's.
        let items = [2, 5, 9];
        assert_eq!(shown_rows(&items, 6, 7), Some((1, 1)));
        assert_eq!(shown_rows(&items, 4, 12), Some((0, 2)));
        assert_eq!(shown_rows(&items, 0, 1), None);
    }

    #[test]
    fn the_preview_is_the_first_plain_line() {
        assert_eq!(first_line("## Plan\nmore", 90), "Plan");
        assert_eq!(
            first_line("```rust\nlet x = 1;\n```\nAfter the code", 90),
            "After the code"
        );
        assert_eq!(
            first_line("- [x] **Done** with [the docs](https://x.y) [2]", 90),
            "Done with the docs"
        );
        assert_eq!(first_line("1. ![chart](a.png) `code`", 90), "chart code");
        assert_eq!(first_line("---\n\n> quoted   text", 90), "quoted text");
        assert_eq!(first_line("abcdefghij", 5), "abcd…");
        assert_eq!(first_line("```\nonly code\n```", 90), "");
    }

    fn drag(from_end: usize, over: f32) -> Drag {
        Drag {
            from_end,
            rows: Cell::new(0),
            size: Cell::new(0),
            drawn: Cell::new(true),
            over,
            slide: None,
        }
    }

    #[test]
    fn window_holds_a_stop_every_pitch() {
        assert_eq!(window_size(0.), 1);
        assert_eq!(window_size(PITCH * 59.), 60);
        assert_eq!(window_size(PITCH * 59. + 9.), 60);
    }

    #[test]
    fn window_centres_on_the_screen_inside_the_rows() {
        assert_eq!(window_start(500, 60, Some((200, 210))), 175);
        assert_eq!(window_start(500, 60, Some((3, 5))), 0);
        assert_eq!(window_start(500, 60, Some((495, 499))), 440);
        // Only the tail on screen: the last window.
        assert_eq!(window_start(500, 60, None), 440);
        assert_eq!(window_start(40, 60, Some((0, 39))), 0);
    }

    #[test]
    fn window_ticks_keep_the_rows_numbers() {
        let users = [true, false, true, false, true];
        let ticks = window_ticks(&users, 2, 2);
        assert_eq!(ticks.len(), 2);
        assert_eq!((ticks[0].first, ticks[0].y, ticks[0].user), (2, 0., true));
        assert_eq!((ticks[1].first, ticks[1].y, ticks[1].user), (3, PITCH, false));
        assert_eq!(window_ticks(&users, 4, 3).len(), 1);
    }

    #[test]
    fn overshoot_has_half_a_pitch_of_slack() {
        let slack = PITCH / 2.;
        assert_eq!(overshoot(-slack, 100.), 0.);
        assert_eq!(overshoot(-slack - 10., 100.), -10.);
        assert_eq!(overshoot(100. + slack, 100.), 0.);
        assert_eq!(overshoot(100. + slack + 30., 100.), 30.);
    }

    #[test]
    fn slide_speeds_up_and_stays_between_the_first_and_last_windows() {
        // Up: further from the end, one more row each ramp past the top, never past the first window.
        assert_eq!(slide(100, 500, 60, -1.), 101);
        assert_eq!(slide(100, 500, 60, -50.), 103);
        assert_eq!(slide(499, 500, 60, -50.), 500);
        // Down: never nearer the end than the last window.
        assert_eq!(slide(100, 500, 60, 1.), 99);
        assert_eq!(slide(61, 500, 60, 50.), 60);
        assert_eq!(slide(60, 500, 60, 50.), 60);
        assert_eq!(slide(100, 500, 60, 0.), 100);
    }

    #[test]
    fn held_window_keeps_its_rows_when_an_older_page_lands_above() {
        // Rows 300 to 360 of 500 shown; a page of 200 lands above: the same rows are now 500 to 560 of 700.
        let held = drag(500 - 300, 0.);
        assert_eq!(held.start(500, 60), 300);
        assert_eq!(held.start(700, 60), 500);
        // The last window reaches the newest rows whatever landed above.
        assert_eq!(drag(60, 0.).start(700, 60), 640);
        // Never past the last window, never before the first row.
        assert_eq!(drag(10, 0.).start(700, 60), 640);
        assert_eq!(drag(900, 0.).start(700, 60), 0);
    }

    #[test]
    fn release_jumps_only_for_a_drag_that_did_not_slide() {
        let jumped = RefCell::new(Vec::new());
        let go = |at: usize| jumped.borrow_mut().push(at);
        let mut state = MinimapState::default();
        // A release with no drag on this list, such as a text selection let go over the rail, does nothing.
        assert!(!release(&mut state, Some(3), true, &go));
        state.drag = Some(drag(100, 0.));
        state.hover = Some(3);
        assert!(release(&mut state, Some(3), true, &go));
        assert!(state.drag.is_none());
        assert_eq!(state.hover, Some(3));
        // Released past an end after a slide: it stays where the slide got to; off the rail, the preview goes.
        state.drag = Some(drag(100, -40.));
        assert!(release(&mut state, Some(0), false, &go));
        assert_eq!(*jumped.borrow(), vec![3]);
        assert_eq!(state.hover, None);
    }
}
