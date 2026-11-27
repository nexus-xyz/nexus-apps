//! Commands scroll below the fixed toolbar, search and decisions, 2px apart as the pane's other lists. The list lays
//! out only the rows near the screen, so a History of thousands of commands scrolls as a short one does.
use super::*;

/// How far past the screen the list lays rows out, so a fast scroll shows no gap.
const OVERDRAW: f32 = 400.;
/// The height a row is taken to have until the list measures it: a closed row and the 2px under it. Without it an
/// unmeasured row counts as 0px, and a fling or a scrollbar drag stops at the last measured row.
const ROW_HINT: f32 = super::rows::ROW + 2.;

/// A row of the list: its command, where that command was in `entries` when the row was made, a digest of what sets
/// the row's height, and the focus inside it (an open shell's output), which keeps the row laid out while it has the
/// keys.
#[derive(Clone)]
pub(super) struct PaneRow {
    pub id: String,
    pub at: usize,
    pub digest: u64,
    pub focus: Option<FocusHandle>,
}

impl PaneRow {
    fn same(&self, other: &Self) -> bool {
        self.id == other.id && self.digest == other.digest && self.focus.is_some() == other.focus.is_some()
    }
}

/// The Commands list and the rows it was last measured for. A row measured once keeps its height until its digest
/// changes, so a read that changes one command measures only that row again.
pub(super) struct PaneList {
    pub(super) state: ListState,
    rows: Vec<PaneRow>,
    /// The list's width when the rows last got the height hint. The list drops every hint on its first layout and
    /// whenever its width changes, so the hint is given again at the next draw.
    hinted: Option<Pixels>,
    /// Whether the list asked for the frame after its first layout.
    asked: bool,
}

impl PaneList {
    pub(super) fn new() -> Self {
        Self {
            state: ListState::new(0, ListAlignment::Top, px(OVERDRAW)),
            rows: Vec::new(),
            hinted: None,
            asked: false,
        }
    }

    /// Give the rows not measured yet the height hint; measured rows keep their heights as theirs. `ListState` is
    /// shared, so this sets the list's own state.
    fn hint(&self) {
        let _ = self.state.clone().with_uniform_item_height(px(ROW_HINT));
    }

    pub(super) fn row(&self, ix: usize) -> Option<&PaneRow> {
        self.rows.get(ix)
    }

    pub(super) fn ids(&self) -> impl Iterator<Item = &str> {
        self.rows.iter().map(|row| row.id.as_str())
    }

    pub(super) fn to_top(&self) {
        self.state.scroll_to(ListOffset::default());
    }

    pub(super) fn reveal(&self, id: &str) {
        if let Some(ix) = self.rows.iter().position(|row| row.id == id) {
            self.state.scroll_to_reveal_item(ix);
        }
    }

    /// Bring the list up to date with `rows`. The rows that match at both ends keep their heights. A middle run of the
    /// same commands is measured again in place, which keeps the reader's place; any other middle run is replaced,
    /// and a row added above the reader moves the place down with it.
    ///
    /// True when the list has not been laid out at a width with the hint yet: the caller then asks for another frame,
    /// after which the hint holds.
    pub(super) fn sync(&mut self, rows: Vec<PaneRow>) -> bool {
        let (old_len, new_len) = (self.rows.len(), rows.len());
        let prefix = self
            .rows
            .iter()
            .zip(&rows)
            .take_while(|(old, new)| old.same(new))
            .count();
        let suffix = self.rows[prefix..]
            .iter()
            .rev()
            .zip(rows[prefix..].iter().rev())
            .take_while(|(old, new)| old.same(new))
            .count();
        let (old_end, new_end) = (old_len - suffix, new_len - suffix);
        if prefix < old_end || prefix < new_end {
            let (old, new) = (&self.rows[prefix..old_end], &rows[prefix..new_end]);
            let in_place = old.len() == new.len()
                && old
                    .iter()
                    .zip(new)
                    .all(|(old, new)| old.id == new.id && old.focus.is_some() == new.focus.is_some());
            if in_place {
                self.state.remeasure_items(prefix..old_end);
            } else {
                self.state
                    .splice_focusable(prefix..old_end, new.iter().map(|row| row.focus.clone()));
                self.hint();
            }
        }
        self.rows = rows;
        let width = self.state.viewport_bounds().size.width;
        if width > px(0.) && self.hinted != Some(width) {
            self.hint();
            self.hinted = Some(width);
        }
        // Once, before the first layout: a list that stays 0px wide does not ask for frames for ever.
        let ask = !self.asked && self.hinted.is_none() && !self.rows.is_empty();
        self.asked |= ask;
        ask
    }
}

/// The pane: the header that stays, and under it `body`, the list or the empty state, which takes the room left.
pub(crate) fn frame(header: Vec<AnyElement>, body: AnyElement) -> AnyElement {
    v_flex()
        .size_full()
        .min_w(px(0.))
        .min_h(px(0.))
        .overflow_hidden()
        .gap(px(10.))
        .child(
            v_flex()
                .debug_selector(|| "commands-pane-header".into())
                .w_full()
                .flex_none()
                .gap(px(8.))
                .children(header),
        )
        .child(
            div()
                .id("commands-pane-list")
                .debug_selector(|| "commands-pane-list".into())
                .flex_1()
                .w_full()
                .min_w(px(0.))
                .min_h(px(0.))
                .child(body),
        )
        .into_any_element()
}

/// A row as the list holds it: full width, 2px above the next.
pub(crate) fn item(row: AnyElement) -> AnyElement {
    div().flex_none().w_full().pb(px(2.)).child(row).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use std::cell::Cell;

    fn rows(ids: &[&str]) -> Vec<PaneRow> {
        ids.iter()
            .enumerate()
            .map(|(at, id)| PaneRow {
                id: (*id).into(),
                at,
                digest: 0,
                focus: None,
            })
            .collect()
    }

    struct Pane {
        list: PaneList,
        count: usize,
        height: f32,
        confirm: bool,
        drawn: Rc<Cell<usize>>,
    }

    impl Pane {
        fn new(count: usize, height: f32) -> Self {
            let mut list = PaneList::new();
            let ids: Vec<String> = (0..count).map(|i| format!("{i}")).collect();
            list.sync(rows(&ids.iter().map(String::as_str).collect::<Vec<_>>()));
            Self {
                list,
                count,
                height,
                confirm: false,
                drawn: Rc::default(),
            }
        }
    }

    impl Render for Pane {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            // As the pane does on each draw (`sync_pane`): the same rows, and the hint after the list's layout.
            let rows = self.list.rows.clone();
            if self.list.sync(rows) {
                window.request_animation_frame();
            }
            let mut header = vec![
                div()
                    .debug_selector(|| "test-command-toolbar".into())
                    .h(px(32.))
                    .child("Active · History · Delete declined · New shell")
                    .into_any_element(),
            ];
            if self.confirm {
                header.push(
                    div()
                        .debug_selector(|| "test-delete-confirmation".into())
                        .h(px(72.))
                        .child("Delete commands? Cancel · Delete")
                        .into_any_element(),
                );
            }
            assert_eq!(self.list.state.item_count(), self.count);
            let body = list(
                self.list.state.clone(),
                cx.processor(|this: &mut Self, i: usize, _, _| {
                    this.drawn.set(this.drawn.get() + 1);
                    item(
                        div()
                            .debug_selector(move || format!("test-history-row-{i}"))
                            .h(px(this.height))
                            .child("Command history")
                            .into_any_element(),
                    )
                }),
            )
            .size_full()
            .into_any_element();
            div().w(px(680.)).h(px(400.)).child(frame(header, body))
        }
    }

    #[gpui_kit::test]
    fn toolbar_and_confirmation_stay_visible_while_history_scrolls(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|_, _| Pane::new(100, 100.));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let toolbar = cx.debug_bounds("test-command-toolbar").unwrap();
        let row = cx.debug_bounds("test-history-row-0").unwrap();
        let list = cx.debug_bounds("commands-pane-list").unwrap();
        cx.simulate_event(ScrollEvent {
            position: list.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(cx.debug_bounds("test-command-toolbar").unwrap(), toolbar);
        let moved = cx.debug_bounds("test-history-row-1").unwrap();
        assert!(
            moved.top() < row.top() + px(100.),
            "the rows moved up under the toolbar"
        );
        view.update(cx, |view, cx| {
            view.confirm = true;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let confirmation = cx.debug_bounds("test-delete-confirmation").unwrap();
        let list = cx.debug_bounds("commands-pane-list").unwrap();
        assert_eq!(cx.debug_bounds("test-command-toolbar").unwrap(), toolbar);
        assert!(confirmation.top() >= toolbar.bottom());
        assert!(confirmation.bottom() <= list.top());
        assert!(list.bottom() <= px(400.));
        // 800px more, drawn after each 200px as a wheel is: the list measures rows as they come near.
        for _ in 0..4 {
            cx.simulate_event(ScrollEvent {
                position: list.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
                ..Default::default()
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
        }
        assert_eq!(cx.debug_bounds("test-delete-confirmation").unwrap(), confirmation);
        assert_eq!(cx.debug_bounds("test-command-toolbar").unwrap(), toolbar);
        view.read_with(cx, |view, _| {
            assert!(
                view.list.state.logical_scroll_top().item_ix >= 8,
                "the history scrolled on"
            )
        });
    }

    /// A History of thousands lays out only the rows near the screen, before and after a long scroll, so a frame
    /// costs the same at any length.
    #[gpui_kit::test]
    fn a_long_history_lays_out_only_the_rows_near_the_screen(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|_, _| Pane::new(5_000, 34.));
        cx.run_until_parked();
        let drawn = view.read_with(cx, |view, _| view.drawn.clone());
        drawn.set(0);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        // The list is under 400px; with 400px of overdraw, about 24 rows of 36px.
        assert!(drawn.get() < 40, "a frame laid out {} rows", drawn.get());
        let list = cx.debug_bounds("commands-pane-list").unwrap();
        // 30 scrolls of 360px, each drawn as a person's wheel is: 10,800px, 300 rows of 36px.
        for _ in 0..30 {
            cx.simulate_event(ScrollEvent {
                position: list.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(-360.))),
                ..Default::default()
            });
            drawn.set(0);
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert!(drawn.get() < 40, "a frame laid out {} rows", drawn.get());
        }
        let top = view.read_with(cx, |view, _| view.list.state.logical_scroll_top().item_ix);
        assert!((290..=310).contains(&top), "the scroll reached row {top}");
        assert!(cx.debug_bounds("test-history-row-0").is_none());
        let below: &'static str = format!("test-history-row-{}", top + 1).leak();
        assert!(cx.debug_bounds(below).is_some());
        // A jump of 1,000 rows, as a scrollbar drag makes, far past the rows measured so far, lands 1,000 rows on:
        // the rows not measured yet count at the hint, not at 0px.
        view.read_with(cx, |view, _| view.list.state.scroll_by(px(36_000.)));
        drawn.set(0);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(drawn.get() < 40, "a frame laid out {} rows", drawn.get());
        let flung = view.read_with(cx, |view, _| view.list.state.logical_scroll_top().item_ix);
        assert!(
            (top + 990..=top + 1_010).contains(&flung),
            "the fling reached row {flung}"
        );
    }

    #[test]
    fn a_new_command_above_the_reader_keeps_the_reader_on_the_same_row() {
        let mut list = PaneList::new();
        list.sync(rows(&["c", "b", "a"]));
        list.state.scroll_to(ListOffset {
            item_ix: 1,
            offset_in_item: px(10.),
        });
        list.sync(rows(&["d", "c", "b", "a"]));
        assert_eq!(list.state.item_count(), 4);
        let top = list.state.logical_scroll_top();
        assert_eq!((top.item_ix, top.offset_in_item), (2, px(10.)), "still on b");
        // A changed row below the reader is measured again in place: the count and the place stay.
        let mut changed = rows(&["d", "c", "b", "a"]);
        changed[3].digest = 1;
        list.sync(changed);
        assert_eq!(list.state.item_count(), 4);
        assert_eq!(list.state.logical_scroll_top().item_ix, 2);
        // A deleted row above leaves, and the reader stays on b.
        let mut deleted = rows(&["c", "b", "a"]);
        deleted[2].digest = 1;
        list.sync(deleted);
        assert_eq!(list.state.item_count(), 3);
        assert_eq!(list.state.logical_scroll_top().item_ix, 1);
        assert_eq!(list.ids().collect::<Vec<_>>(), ["c", "b", "a"]);
    }
}
