//! A command's text copies with the copy keys while the command's shell has the focus, and the shell gets no Ctrl+C.

use super::Terminal;
use super::render::command_text;
use crate::palette::Palette;
use core::prelude::v1::test;
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};

const COMMAND: &str = "read -p 'Enter a word: ' word";

/// A card's command above the command's shell, both in an element that tracks the focus, as the workspace does
/// (`workspace/mod.rs`, its `track_focus`).
struct Probe {
    shell: Entity<Terminal>,
    focus: FocusHandle,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = palette();
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .child(command_text("one", COMMAND, palette))
            .child(div().h(px(120.)).child(self.shell.clone()))
    }
}

fn palette() -> Palette {
    Palette::for_choice(crate::palette::ThemeChoice::Light, WindowAppearance::Light)
}

#[gpui_kit::test]
fn command_text_copies_while_its_shell_has_the_focus(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let sent: Rc<RefCell<Vec<(String, Value)>>> = Rc::default();
    let record = sent.clone();
    let window = cx.add_window(move |window, cx| {
        // The terminal reads the theme the workspace sets on each frame.
        cx.set_global(crate::pages::Theme(palette()));
        let probe = cx.new(|cx| {
            let shell = cx.new(|cx| {
                let send: Rc<dyn Fn(&str, Value)> =
                    Rc::new(move |op, args| record.borrow_mut().push((op.into(), args)));
                Terminal::attached(send, cx)
            });
            shell.update(cx, |shell, cx| {
                shell.feed(&[], true, false, cx);
                window.focus(shell.focus_handle(), cx);
            });
            Probe {
                shell,
                focus: cx.focus_handle(),
            }
        });
        gpui_kit::base::Root::new(probe, window, cx)
    });
    let probe = window.update(cx, |root, _, _| root.view().clone()).unwrap();
    let probe = probe.downcast::<Probe>().unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    let shell_focused = |window: &Window, cx: &App| probe.read(cx).shell.read(cx).focus_handle().is_focused(window);
    cx.update(|window, cx| {
        window.activate_window();
        window.draw(cx).clear(cx);
        assert!(shell_focused(window, cx), "the shell starts with the focus");
    });
    let card = cx.debug_bounds("command-text").expect("the command is drawn");
    // Inside the card's 12px padding, along the command's one line.
    let (start, end) = (
        card.origin + point(px(13.), px(21.)),
        point(card.right() - px(13.), card.origin.y + px(21.)),
    );
    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::none());
    cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        assert_eq!(gpui_kit::base::TextSelection::selected_text(window, cx), COMMAND);
        assert!(!shell_focused(window, cx), "the press took the focus from the shell");
    });
    cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-c" } else { "ctrl-c" });
    cx.update(|_, cx| {
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
            Some(COMMAND)
        )
    });
    let inputs: Vec<_> = sent.borrow().iter().filter(|(op, _)| op == "input").cloned().collect();
    assert!(inputs.is_empty(), "the shell got no input: {inputs:?}");
}
