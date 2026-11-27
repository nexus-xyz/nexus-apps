//! A draft session's shells in the Commands look: the grip drags a well to a whole line, and Maximize gives one well
//! the pane.
use core::prelude::v1::test;

use gpui_kit::*;

use super::TerminalSet;
use crate::palette::{Palette, ThemeChoice};
use crate::side::well;

struct Host(Entity<TerminalSet>);

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(480.)).h(px(760.)).child(self.0.clone())
    }
}

fn open(cx: &mut TestAppContext) -> (Entity<TerminalSet>, &mut VisualTestContext) {
    cx.update(gpui_kit::init);
    // The motion is `motion::`'s own tests' to prove: here each move lands at once.
    cx.update(|cx| cx.set_reduce_motion(true));
    let (host, cx) = cx.add_window_view(|_, cx| {
        cx.set_global(crate::pages::Theme(Palette::for_choice(
            ThemeChoice::Light,
            WindowAppearance::Light,
        )));
        Host(cx.new(|cx| {
            let mut set = TerminalSet::for_view_test(std::env::temp_dir(), cx);
            set.set_commands_look(true, cx);
            set
        }))
    });
    let set = host.read_with(cx, |host, _| host.0.clone());
    // Two frames: the first paints the pane's foot and the well's top, which the second fits to.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (set, cx)
}

fn well_height(cx: &mut VisualTestContext) -> f32 {
    f32::from(cx.debug_bounds("well-draft-1").expect("the well drew").size.height)
}

#[gpui_kit::test]
fn the_grip_drags_the_well_to_a_whole_line(cx: &mut TestAppContext) {
    let (set, cx) = open(cx);
    assert_eq!(well_height(cx), well::height(well::DEFAULT_LINES));
    let grip = cx.debug_bounds("well-grip-draft-1").expect("the grip drew").center();
    let to = point(grip.x, grip.y + px(100.));
    cx.simulate_mouse_down(grip, MouseButton::Left, Modifiers::none());
    assert!(
        set.read_with(cx, |set, _| set.wells.dragging()),
        "the press began a drag"
    );
    // The press redrew the well: the grip must keep its press through that, or GPUI's drag never starts.
    cx.simulate_mouse_move(point(grip.x, grip.y + px(20.)), MouseButton::Left, Modifiers::none());
    assert!(cx.update(|_, cx| cx.has_active_drag()), "the move started GPUI's drag");
    cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // While it drags, the well follows the pointer, between lines.
    assert_eq!(well_height(cx), well::height(well::DEFAULT_LINES) + 100.);
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    // Let go, it rests on the nearest line: 100px is 5.6 lines, so 18 lines.
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(well_height(cx), well::height(18));
}

#[gpui_kit::test]
fn maximize_gives_one_well_the_pane(cx: &mut TestAppContext) {
    let (set, cx) = open(cx);
    set.update(cx, |set, cx| set.add(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("draft-command-row-1").is_some());
    cx.debug_bounds("well-maximize-draft-2")
        .expect("the newest row has Maximize");
    set.update(cx, |set, cx| {
        set.wells.toggle_maximized(&"draft-2".into());
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // The other row hides, and the well grows toward the pane's foot.
    assert!(cx.debug_bounds("draft-command-row-1").is_none());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let grown = f32::from(cx.debug_bounds("well-draft-2").unwrap().size.height);
    assert!(grown > well::height(30), "the well took the pane: {grown}px");
    // Restore gives it back.
    set.update(cx, |set, cx| {
        set.wells.restore();
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("draft-command-row-1").is_some());
}
