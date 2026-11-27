//! A rich block's corner: hidden at rest, over the drawing under the pointer, in the code's head when the code shows;
//! Copy takes the code, and the web view under it has no background.

use gpui_kit::{
    AppContext, Context, Entity, IntoElement, Modifiers, MouseButton, ParentElement, Render, Styled, TestAppContext,
    VisualTestContext, Window, WindowAppearance, div, px,
};

use super::RichBlock;
use crate::palette::{Palette, ThemeChoice};

const SOURCE: &str = "Grows like $2^n$, past $n^3$.";

struct Answer {
    block: Entity<RichBlock>,
}

impl Render for Answer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Room above the block for its floating corner, as the answer's earlier lines give it.
        div().w(px(640.)).pt(px(64.)).child(self.block.clone())
    }
}

fn answer(cx: &mut TestAppContext) -> (&mut VisualTestContext, Entity<RichBlock>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(crate::pages::Theme(Palette::for_choice(
            ThemeChoice::Dark,
            WindowAppearance::Dark,
        )));
        // The corner's fade jumps to its end, so one draw shows where it lands.
        cx.set_reduce_motion(true);
    });
    let mut block = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let made = cx.new(|cx| RichBlock::new(SOURCE.into(), SOURCE.into(), false, window, cx));
        block = Some(made.clone());
        Answer { block: made }
    });
    cx.run_until_parked();
    draw(cx);
    (cx, block.expect("the block is made"))
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is drawn"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
    draw(cx);
}

fn hover_area(cx: &mut VisualTestContext) {
    let area = cx.debug_bounds("rich-area").expect("the drawing's area is drawn");
    cx.simulate_mouse_move(area.center(), None, Modifiers::none());
    cx.run_until_parked();
    draw(cx);
}

#[gpui_kit::test]
fn the_page_has_no_background_and_its_words_show_until_it_draws(cx: &mut TestAppContext) {
    let (cx, block) = answer(cx);
    block.read_with(cx, |block, cx| {
        let web = block.web.as_ref().expect("a page opens");
        assert!(web.read(cx).is_clear(), "the transcript's surface shows through");
        assert_eq!(block.placeholder.as_ref(), "Grows like 2^n, past n^3.");
    });
    assert!(cx.debug_bounds("rich-placeholder").is_some());
}

#[gpui_kit::test]
fn the_corner_is_gone_at_rest_and_floats_above_the_drawing_under_the_pointer(cx: &mut TestAppContext) {
    let (cx, _) = answer(cx);
    assert!(cx.debug_bounds("rich-toggle").is_none(), "no corner at rest");
    hover_area(cx);
    let corner = cx
        .debug_bounds("rich-corner")
        .expect("the corner shows under the pointer");
    let area = cx.debug_bounds("rich-area").unwrap();
    assert!(corner.bottom() <= area.top(), "it never covers the block's own words");
    assert!((corner.right() - area.right()).abs() <= px(4.), "at the top right");
    assert!(cx.debug_bounds("rich-toggle").is_some());
    assert!(cx.debug_bounds("code-copy").is_some());
}

#[gpui_kit::test]
fn show_latex_opens_the_code_with_its_corner_and_copy_takes_it(cx: &mut TestAppContext) {
    let (cx, block) = answer(cx);
    hover_area(cx);
    click(cx, "rich-toggle");
    assert!(block.read_with(cx, |block, _| block.show_code));
    let well = cx.debug_bounds("rich-code").expect("the code shows in its well");
    let corner = cx
        .debug_bounds("rich-corner")
        .expect("the corner stays in the well's head");
    assert!(corner.top() >= well.top() && corner.right() <= well.right());
    // Copy keeps the same hitbox and pending click when the fade ends between the mouse events.
    let copy = cx.debug_bounds("code-copy").expect("Copy is drawn");
    cx.simulate_mouse_down(copy.center(), MouseButton::Left, Modifiers::none());
    cx.executor().advance_clock(crate::motion::UI);
    draw(cx);
    assert_eq!(cx.debug_bounds("code-copy"), Some(copy));
    cx.simulate_mouse_up(copy.center(), MouseButton::Left, Modifiers::none());
    draw(cx);
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some(SOURCE));
    click(cx, "rich-toggle");
    assert!(!block.read_with(cx, |block, _| block.show_code), "Show math goes back");
}

#[gpui_kit::test]
fn a_failed_drawing_keeps_the_code_open_with_retry(cx: &mut TestAppContext) {
    let (cx, block) = answer(cx);
    block.update(cx, |block, cx| {
        block.failed = true;
        cx.notify();
    });
    draw(cx);
    assert!(cx.debug_bounds("rich-code").is_some());
    assert!(cx.debug_bounds("rich-retry").is_some());
    assert!(cx.debug_bounds("rich-toggle").is_none(), "nothing drew to go back to");
}
