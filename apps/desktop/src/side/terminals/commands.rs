//! A session's shells before it starts, drawn as the Commands tab it has once it starts (`shared_terminals/view.rs`):
//! the same toolbar, empty state and rows. A draft has no stored session, so its shells cannot be shared commands
//! yet. Each terminal is one row; open, it shows its shells side by side in the row's output well.

use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{MAX_SPLIT, TerminalSet};
use crate::pages::ui;
use crate::palette::Palette;
use crate::side::INSET;
use crate::side::shared_terminals::{ROW, ROW_RADIUS, pane_frame, pane_item, row_button};
use crate::side::well;
use crate::workspace::glyph;

/// What New shell does, from the toolbar or the empty state.
pub type NewShell = Rc<dyn Fn(&mut Window, &mut App)>;

/// The detail under "No commands running": the agent's commands come once the session starts.
pub const EMPTY_DETAIL: &str =
    "Commands the agent runs show here once the session starts. Open a shell of your own with New shell.";

/// The pane with no shell: the toolbar and the empty state, as the Commands tab with no command.
pub fn render_empty(new_shell: NewShell, palette: Palette) -> AnyElement {
    let again = new_shell.clone();
    let body = div()
        .id("commands-pane-empty")
        .size_full()
        .overflow_y_scroll()
        .child(
            ui::empty_state(Lucide::SquareTerminal, "No commands running", EMPTY_DETAIL, palette).child(
                ui::button("commands-empty-shell", "New shell", palette)
                    .on_click(move |_, window, cx| again(window, cx)),
            ),
        )
        .into_any_element();
    inset(pane_frame(vec![toolbar(0, new_shell, palette)], body))
}

/// The Active tab, alone as the Commands tab's while History is off, and New shell.
fn toolbar(active: usize, new_shell: NewShell, palette: Palette) -> AnyElement {
    h_flex()
        .gap(px(8.))
        .child(ui::tabs(palette).child(ui::tab("commands-active", format!("Active {active}"), true, palette)))
        .child(div().flex_1())
        .child(
            ui::ghost_button("commands-native-shell", "New shell", palette)
                .on_click(move |_, window, cx| new_shell(window, cx)),
        )
        .into_any_element()
}

/// The pane's inset from the Workbench's edges, as `render_commands_body` gives the Commands tab.
fn inset(pane: AnyElement) -> AnyElement {
    div()
        .id("native-commands-pane")
        .size_full()
        .min_h(px(0.))
        .overflow_hidden()
        .px(px(INSET))
        .pt(px(INSET))
        .child(pane)
        .into_any_element()
}

/// The well key of terminal `id`'s row (`side::well`).
pub(super) fn well_key(id: usize) -> SharedString {
    SharedString::from(format!("draft-{id}"))
}

fn wells(set: &mut TerminalSet) -> &mut well::Wells {
    &mut set.wells
}

/// What drags a divider between two shells of a split: the terminal's id and the shell before the divider.
#[derive(Clone)]
struct DividerDrag {
    group: usize,
    divider: usize,
}

impl Render for DividerDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

impl TerminalSet {
    pub(super) fn render_commands(&mut self, palette: Palette, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let set = cx.entity().downgrade();
        let new_shell: NewShell = Rc::new(move |window, cx| {
            if let Some(set) = set.upgrade() {
                set.update(cx, |set, cx| {
                    set.wells.restore();
                    set.add(cx);
                    set.focus(window, cx);
                });
            }
        });
        if self.groups.is_empty() {
            return render_empty(new_shell, palette);
        }
        let motion = !cx.reduce_motion();
        let maximized = self
            .wells
            .maximized()
            .and_then(|key| self.groups.iter().position(|group| &well_key(group.id) == key));
        // Newest first, as the Commands tab lists; a maximized well's row alone.
        let rows: Vec<AnyElement> = match maximized {
            Some(ix) => vec![pane_item(self.render_command_row(ix, motion, palette, window, cx))],
            None => (0..self.groups.len())
                .rev()
                .map(|ix| pane_item(self.render_command_row(ix, motion, palette, window, cx)))
                .collect(),
        };
        let list = v_flex().w_full().children(rows);
        // The other rows fade back in once after a maximized well gives the pane back.
        let list = match (maximized, self.wells.restores()) {
            (None, Some(restore)) if motion => {
                crate::motion::fade_in(list, SharedString::from(format!("draft-restore-{restore}"))).into_any_element()
            }
            _ => list.into_any_element(),
        };
        // The probe stays outside the scroll, so it notes the pane's visible foot.
        let body = div()
            .relative()
            .size_full()
            .child(self.wells.pane_probe())
            .child(
                div()
                    .id("commands-pane-rows")
                    .size_full()
                    .when(maximized.is_none(), |body| body.overflow_y_scroll())
                    .child(list),
            )
            .into_any_element();
        inset(pane_frame(vec![toolbar(self.groups.len(), new_shell, palette)], body))
    }

    /// A terminal as a command's row: chevron, running mark, name, "Interactive shell", Split, Maximize and Stop;
    /// open, its shells in their well.
    fn render_command_row(
        &self,
        ix: usize,
        motion: bool,
        palette: Palette,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group = &self.groups[ix];
        let id = group.id;
        let key = well_key(id);
        let open = !self.collapsed.contains(&id);
        let split = group.shells.len();
        let maximized = self.wells.is_maximized(&key);
        let row = h_flex()
            .id(("draft-command-row", id))
            .debug_selector(move || format!("draft-command-row-{id}"))
            .group("command-row")
            .w_full()
            .h(px(ROW))
            .flex_none()
            .px(px(8.))
            .gap(px(8.))
            .rounded(px(ROW_RADIUS))
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .active(|style| style.opacity(0.8))
            .child(
                glyph(
                    if open {
                        Lucide::ChevronDown
                    } else {
                        Lucide::ChevronRight
                    },
                    palette.faint,
                )
                .size(px(12.))
                .flex_none(),
            )
            .child(running_mark(id, motion, palette))
            .child(
                div()
                    .flex_none()
                    .max_w(relative(0.45))
                    .min_w(px(0.))
                    .truncate()
                    .text_size(px(13.))
                    .text_color(palette.foreground)
                    .child(group.name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .font_family(ui::MONO)
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child("Interactive shell"),
            )
            .when(split > 1, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(palette.faint)
                        .child(format!("{split} shells")),
                )
            })
            .when(open && split < MAX_SPLIT, |row| {
                row.child(
                    well::icon_button(
                        SharedString::from(format!("draft-split-{id}")),
                        Lucide::Columns2,
                        "Split",
                        palette,
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.split_group(ix, window, cx);
                    })),
                )
            })
            .when(open, |row| {
                let key = key.clone();
                row.child(
                    well::maximize_button(&key.clone(), maximized, palette).on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.wells.toggle_maximized(&key);
                            cx.notify();
                        },
                    )),
                )
            })
            .child(
                row_button(&format!("draft-{id}"), "stop", "Stop", false, true, palette).on_click(cx.listener(
                    move |this, _, window, cx| {
                        // The row under it opens and closes on a click; this one only acts.
                        cx.stop_propagation();
                        this.close_group(ix, window, cx);
                    },
                )),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.collapsed.remove(&id) {
                    this.choose(ix, window, cx);
                } else {
                    this.collapsed.insert(id);
                    if this.wells.is_maximized(&well_key(id)) {
                        this.wells.restore();
                    }
                    cx.notify();
                }
            }));
        if !open {
            return row.into_any_element();
        }
        let hidden = if maximized { self.groups.len() - 1 } else { 0 };
        let details = v_flex()
            .w_full()
            .min_w(px(0.))
            .gap(px(8.))
            .pl(px(well::INDENT))
            .pr(px(8.))
            .pt(px(2.))
            .pb(px(well::FOOT))
            .child(
                h_flex()
                    .items_start()
                    .gap(px(4.))
                    .child(ui::note("Folder:", palette))
                    .child(ui::note(self.dir.display().to_string(), palette).flex_1().min_w(px(0.)))
                    .children(well::hidden_note(hidden, palette)),
            )
            .child(self.render_well(ix, palette, window, cx));
        v_flex()
            .w_full()
            .min_w(px(0.))
            .child(row)
            .child(details)
            .into_any_element()
    }

    /// Terminal `ix`'s shells in their well, side by side on hairlines that drag.
    fn render_well(&self, ix: usize, palette: Palette, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let group = &self.groups[ix];
        let id = group.id;
        let shares = self.shares_of(group);
        let focused = group
            .shells
            .iter()
            .position(|shell| shell.read(cx).focus_handle().contains_focused(window, cx));
        let size = group
            .shells
            .get(focused.unwrap_or(group.focused))
            .map(|shell| shell.read(cx).size());
        // A column's width, for the dividers' whole-column steps: the shells share one font.
        let cell = group.shells.first().and_then(|shell| shell.read(cx).cell_width());
        let mut shells: Vec<AnyElement> = Vec::new();
        for (at, shell) in group.shells.iter().enumerate() {
            if at > 0 {
                shells.push(divider(id, at - 1, palette));
            }
            let cell = div()
                .h_full()
                .min_w(px(0.))
                .w(relative(shares[at]))
                .child(shell.clone());
            shells.push(if self.arrived == Some(shell.entity_id()) {
                let arrive = SharedString::from(format!("split-arrive-{}", shell.entity_id()));
                crate::motion::slide_in(cell, arrive).into_any_element()
            } else {
                cell.into_any_element()
            });
        }
        let body = h_flex()
            .id(SharedString::from(format!("draft-shells-{id}")))
            .size_full()
            .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<DividerDrag>, _, cx| {
                let drag = event.drag(cx).clone();
                if drag.group != id {
                    return;
                }
                let (Some(cell), bounds) = (cell, event.bounds) else {
                    return;
                };
                let x = f32::from(event.event.position.x - bounds.left());
                this.drag_divider(id, drag.divider, x, f32::from(bounds.size.width), cell, cx);
            }))
            .children(shells)
            .into_any_element();
        let look = well::Look {
            focused: focused.is_some(),
            size,
        };
        well::render(well_key(id), &self.wells, wells, body, look, palette, cx)
    }
}

/// The hairline between two shells of a split, with an 8px strip over it that drags it.
fn divider(group: usize, divider: usize, palette: Palette) -> AnyElement {
    div()
        .relative()
        .w(px(1.))
        .h_full()
        .flex_none()
        .bg(palette.border)
        .child(
            div()
                .id(SharedString::from(format!("draft-divider-{group}-{divider}")))
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(-4.))
                .w(px(9.))
                .cursor_col_resize()
                .occlude()
                .on_drag(DividerDrag { group, divider }, |drag, _, _, cx| {
                    cx.new(|_| drag.clone())
                }),
        )
        .into_any_element()
}

/// A running shell's mark: the Commands tab's dot, breathing at 30 frames a second unless motion is reduced.
fn running_mark(id: usize, motion: bool, palette: Palette) -> AnyElement {
    let dot = div().size(px(7.)).rounded_full().bg(palette.foreground);
    let dot = if motion {
        dot.with_animation(
            SharedString::from(format!("draft-command-breathe-{id}")),
            Animation::new(crate::motion::PULSE)
                .repeat()
                .with_max_fps(crate::motion::LOOP_FPS)
                .with_easing(pulsating_between(0.35, 1.0)),
            |dot, delta| dot.opacity(delta),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    };
    div()
        .size(px(16.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(dot)
        .into_any_element()
}
