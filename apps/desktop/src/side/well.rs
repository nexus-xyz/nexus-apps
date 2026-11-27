//! The well a shell sits in, in the Commands tab: a draft session's shells (`terminals/commands.rs`) and a command's
//! output (`shared_terminals/rows.rs`). It is sized in whole lines of the terminal, 12 by default. A grip on its bottom
//! edge drags its height, with the shell's size read out while it drags; a double click on the grip switches between
//! 12 lines and the pane's height. The row's Maximize gives the well the whole pane. Each row keeps its height for the
//! session. The motion is `motion::`'s: the grip fades (`fade_to`), a drag settles on its last line (`SETTLE`), and
//! Maximize grows the well (`PANEL`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::terminal::{LINE_HEIGHT, PADDING, output_well};
use crate::motion::{PANEL, SETTLE, ease_height, fade_to};
use crate::pages::ui;
use crate::palette::Palette;
use crate::workspace::glyph;

/// A new well's height, in lines, and the shortest a drag leaves it.
pub(crate) const DEFAULT_LINES: u16 = 12;
pub(crate) const MIN_LINES: u16 = 6;
/// A row's radius (DESIGN.md: 6 for controls and rows).
pub(crate) const RADIUS: f32 = 6.;
/// The room under a well inside its row: what a maximized well leaves at the pane's foot.
pub(crate) const FOOT: f32 = 12.;
/// The well's content starts here, past the row's chevron: the row's details share this inset.
pub(crate) const INDENT: f32 = 32.;
/// The grip's hit strip on the well's bottom edge, and its pill.
const GRIP: f32 = 8.;
const PILL: (f32, f32) = (32., 4.);
/// The pill's opacity under the pointer and while it drags; at rest it does not show.
const PILL_HOVER: f32 = 0.5;
const PILL_DRAG: f32 = 0.9;

/// A well's height for `lines` lines of the terminal and its insets.
pub(crate) fn height(lines: u16) -> f32 {
    f32::from(lines) * LINE_HEIGHT + 2. * PADDING
}

/// The whole lines that fit in `height`, at least [`MIN_LINES`].
fn lines_in(height: f32) -> u16 {
    ((height - 2. * PADDING) / LINE_HEIGHT)
        .floor()
        .max(f32::from(MIN_LINES)) as u16
}

/// The line count nearest a dragged `height`, from [`MIN_LINES`] to `most`.
fn nearest(height: f32, most: u16) -> u16 {
    let lines = ((height - 2. * PADDING) / LINE_HEIGHT).round();
    lines.clamp(f32::from(MIN_LINES), f32::from(most.max(MIN_LINES))) as u16
}

/// What the grip carries while it drags: the well it sizes.
#[derive(Clone)]
pub(crate) struct WellDrag(SharedString);

impl Render for WellDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

struct Drag {
    key: SharedString,
    /// The pointer's y and the well's height when the drag began, and the height now.
    start_y: f32,
    start: f32,
    now: f32,
}

/// A drag's end: the well eases from where it was let go to its whole line.
struct Settle {
    key: SharedString,
    at: Instant,
}

/// The wells of one pane: their heights, the drag, the hovered well and the maximized one. One owner (the pane's
/// view) changes it, through these functions.
#[derive(Default)]
pub(crate) struct Wells {
    lines: HashMap<SharedString, u16>,
    drag: Option<Drag>,
    settle: Option<Settle>,
    hovered: Option<SharedString>,
    maximized: Option<SharedString>,
    /// How often a maximized well gave the pane back: each restore fades the other rows in once.
    restores: u64,
    /// The pane's foot and each well's top, in window pixels, as last painted: what fits a well to the pane.
    foot: Rc<Cell<Option<f32>>>,
    tops: Rc<RefCell<HashMap<SharedString, f32>>>,
}

impl Wells {
    /// The room from a well's top to the pane's foot, less [`FOOT`]. None before both were painted.
    fn room(&self, key: &str) -> Option<f32> {
        let top = *self.tops.borrow().get(key)?;
        Some(self.foot.get()? - top - FOOT)
    }

    /// The most lines a well may take: what fits the pane, or no limit before the pane was painted.
    fn most(&self, key: &str) -> u16 {
        self.room(key).map_or(u16::MAX, lines_in)
    }

    fn lines(&self, key: &str) -> u16 {
        self.lines.get(key).copied().unwrap_or(DEFAULT_LINES)
    }

    /// The height a well rests at: its lines, or the pane's room while maximized.
    fn target(&self, key: &str) -> f32 {
        let lines = height(self.lines(key).min(self.most(key)));
        if self.is_maximized(key) {
            return self.room(key).unwrap_or(lines).max(lines);
        }
        lines
    }

    #[cfg(test)]
    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub(crate) fn is_maximized(&self, key: &str) -> bool {
        self.maximized.as_deref() == Some(key)
    }

    pub(crate) fn maximized(&self) -> Option<&SharedString> {
        self.maximized.as_ref()
    }

    /// Maximize `key`'s well, or give the pane back.
    pub(crate) fn toggle_maximized(&mut self, key: &SharedString) {
        if self.is_maximized(key) {
            self.restore();
        } else {
            self.settle = None;
            self.maximized = Some(key.clone());
        }
    }

    /// Give the pane back. True when a well was maximized.
    pub(crate) fn restore(&mut self) -> bool {
        if self.maximized.take().is_none() {
            return false;
        }
        self.restores += 1;
        true
    }

    /// The restore that the other rows fade in after, while there was one.
    pub(crate) fn restores(&self) -> Option<u64> {
        (self.restores > 0).then_some(self.restores)
    }

    fn begin(&mut self, key: SharedString, y: f32) {
        let start = self.target(&key);
        self.settle = None;
        self.drag = Some(Drag {
            key,
            start_y: y,
            start,
            now: start,
        });
    }

    /// Follow the pointer, from [`MIN_LINES`] to what fits the pane. True when the height changed.
    fn drag_to(&mut self, key: &str, y: f32) -> bool {
        let most = height(self.most(key));
        let Some(drag) = self.drag.as_mut().filter(|drag| drag.key == key) else {
            return false;
        };
        let now = (drag.start + y - drag.start_y).clamp(height(MIN_LINES), most.max(height(MIN_LINES)));
        std::mem::replace(&mut drag.now, now) != now
    }

    /// Let go: the well keeps the nearest whole line, and eases there. True when a drag ended.
    fn release(&mut self) -> bool {
        let Some(drag) = self.drag.take() else {
            return false;
        };
        let lines = nearest(drag.now, self.most(&drag.key));
        self.lines.insert(drag.key.clone(), lines);
        self.settle = Some(Settle {
            key: drag.key,
            at: Instant::now(),
        });
        true
    }

    /// A double click on the grip: 12 lines, or what fits the pane.
    fn fit(&mut self, key: &SharedString) {
        self.settle = None;
        let most = self.most(key);
        let lines = if self.lines(key) == DEFAULT_LINES && most != u16::MAX {
            most
        } else {
            DEFAULT_LINES
        };
        self.lines.insert(key.clone(), lines.max(MIN_LINES));
    }

    /// The pointer came over a well or left it. True when that changes what shows.
    fn hover(&mut self, key: &SharedString, hovered: bool) -> bool {
        let next = if hovered {
            Some(key.clone())
        } else if self.hovered.as_ref() == Some(key) {
            None
        } else {
            return false;
        };
        std::mem::replace(&mut self.hovered, next) != self.hovered
    }

    /// How a resting well moves to its height: from where a drag let go (`SETTLE`), else as the pane changes
    /// (`PANEL`).
    fn course(&self, key: &str) -> (f32, Duration) {
        let target = self.target(key);
        match &self.settle {
            Some(settle) if settle.key == key && settle.at.elapsed() < SETTLE => (target, SETTLE),
            _ => (target, PANEL),
        }
    }

    /// An empty box, laid over the pane's body, that notes where the pane ends.
    pub(crate) fn pane_probe(&self) -> impl IntoElement {
        let foot = self.foot.clone();
        canvas(
            move |bounds, _, _| foot.set(Some(f32::from(bounds.bottom()))),
            |_, _, _, _| {},
        )
        .absolute()
        .size_full()
    }
}

/// How a well draws beside its key and its body.
pub(crate) struct Look {
    /// One of its shells has the keys: the well takes a 1px ring.
    pub focused: bool,
    /// The focused shell's columns and rows, read out while the grip drags.
    pub size: Option<(u16, u16)>,
}

/// The well `key` around `body` (its shells), at its height. `pick` finds the pane's [`Wells`] in its view.
pub(crate) fn render<V: 'static>(
    key: SharedString,
    wells: &Wells,
    pick: fn(&mut V) -> &mut Wells,
    body: AnyElement,
    look: Look,
    palette: Palette,
    cx: &mut Context<V>,
) -> AnyElement {
    let maximized = wells.is_maximized(&key);
    let dragged = wells.drag.as_ref().filter(|drag| drag.key == key).map(|drag| drag.now);
    let pill = match (dragged.is_some(), wells.hovered.as_ref() == Some(&key)) {
        (true, _) => PILL_DRAG,
        (false, true) => PILL_HOVER,
        (false, false) => 0.,
    };
    let tops = wells.tops.clone();
    let measured = key.clone();
    let well = div()
        .id(SharedString::from(format!("well-{key}")))
        .debug_selector({
            let key = key.clone();
            move || format!("well-{key}")
        })
        .relative()
        .w_full()
        .flex_none()
        .overflow_hidden()
        .rounded(px(RADIUS))
        // Always 1px, so focus changes no layout; clear while no shell in it has the keys.
        .border_1()
        .border_color(if look.focused {
            palette.icon.opacity(0.4)
        } else {
            transparent_black()
        })
        .bg(output_well(palette))
        .on_hover(cx.listener({
            let key = key.clone();
            move |this, hovered: &bool, _, cx| {
                if pick(this).hover(&key, *hovered) {
                    cx.notify();
                }
            }
        }))
        .child(
            canvas(
                move |bounds, _, _| {
                    tops.borrow_mut().insert(measured, f32::from(bounds.top()));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .child(div().size_full().child(body))
        .when(!maximized, |well| {
            well.child(grip(key.clone(), pill, pick, palette, cx))
        })
        .when_some(dragged.and(look.size), |well, (cols, rows)| {
            well.child(readout(cols, rows, palette))
        });
    // One tween for every state, so the grip under it keeps its press while it drags: the drag follows the pointer
    // with no easing, then settles; Maximize and fit ease over a panel's time.
    let (height, duration) = match dragged {
        Some(now) => (now, Duration::ZERO),
        None => wells.course(&key),
    };
    ease_height(well, SharedString::from(format!("well-height-{key}")), height, duration).into_any_element()
}

/// The grip: an 8px strip on the well's bottom edge, with a pill that shows on hover and while it drags.
fn grip<V: 'static>(
    key: SharedString,
    pill: f32,
    pick: fn(&mut V) -> &mut Wells,
    palette: Palette,
    cx: &mut Context<V>,
) -> impl IntoElement {
    let release = move |this: &mut V, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<V>| {
        if pick(this).release() {
            cx.notify();
        }
    };
    let mark = div()
        .w(px(PILL.0))
        .h(px(PILL.1))
        .rounded(px(PILL.1 / 2.))
        .bg(palette.icon);
    div()
        .id(SharedString::from(format!("well-grip-{key}")))
        .debug_selector({
            let key = key.clone();
            move || format!("well-grip-{key}")
        })
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .h(px(GRIP))
        .flex()
        .items_center()
        .justify_center()
        .cursor_row_resize()
        // The shell under the strip does not take the press as a selection.
        .occlude()
        .tooltip(|window, cx| Tooltip::new("Drag to resize; double-click to fit the pane").build(window, cx))
        .child(fade_to(mark, SharedString::from(format!("well-pill-{key}")), pill))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener({
                let key = key.clone();
                move |this, event: &MouseDownEvent, _, cx| {
                    let wells = pick(this);
                    if event.click_count >= 2 {
                        wells.fit(&key);
                    } else {
                        wells.begin(key.clone(), f32::from(event.position.y));
                    }
                    cx.notify();
                }
            }),
        )
        .on_drag(WellDrag(key.clone()), |drag, _, _, cx| cx.new(|_| drag.clone()))
        .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<WellDrag>, _, cx| {
            if event.drag(cx).0 == key && pick(this).drag_to(&key, f32::from(event.event.position.y)) {
                cx.notify();
            }
        }))
        .on_mouse_up(MouseButton::Left, cx.listener(release))
        .on_mouse_up_out(MouseButton::Left, cx.listener(release))
}

/// The shell's size while the grip drags, as terminals show it when their window resizes.
fn readout(cols: u16, rows: u16, palette: Palette) -> impl IntoElement {
    div()
        .absolute()
        .right(px(8.))
        .bottom(px(FOOT))
        .px(px(6.))
        .py(px(2.))
        .rounded(px(4.))
        .bg(palette.raised)
        .font_family(ui::MONO)
        .text_size(px(11.))
        .text_color(palette.muted)
        .child(format!("{cols} × {rows}"))
}

/// A row's icon button for its well (Maximize, Split): 24px square, its icon 14px.
pub(crate) fn icon_button(id: SharedString, icon: Lucide, tip: &'static str, palette: Palette) -> Stateful<Div> {
    div()
        .id(id.clone())
        .debug_selector(move || id.to_string())
        .size(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .cursor_pointer()
        .hover(|style| style.bg(palette.selected))
        .active(|style| style.opacity(0.8))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(glyph(icon, palette.icon).size(px(14.)))
}

/// A row's Maximize, or Restore while its well has the pane.
pub(crate) fn maximize_button(key: &str, maximized: bool, palette: Palette) -> Stateful<Div> {
    let (icon, tip) = if maximized {
        (Lucide::Minimize2, "Restore")
    } else {
        (Lucide::Maximize2, "Maximize")
    };
    icon_button(SharedString::from(format!("well-maximize-{key}")), icon, tip, palette)
}

/// The note beside a maximized well's folder: how many rows it hides.
pub(crate) fn hidden_note(hidden: usize, palette: Palette) -> Option<impl IntoElement> {
    let words = match hidden {
        0 => return None,
        1 => "1 more command hidden".to_string(),
        n => format!("{n} more commands hidden"),
    };
    Some(ui::note(words, palette).flex_none())
}

#[cfg(test)]
#[path = "well_tests.rs"]
mod tests;
