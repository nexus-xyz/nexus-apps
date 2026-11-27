//! Headless native interaction tests. These draw the actual controls, without touching the running app.
use super::*;
use core::prelude::v1::test;
use std::{cell::RefCell, rc::Rc};

struct Controls {
    clicks: Rc<RefCell<Vec<&'static str>>>,
    palette: Palette,
}

impl Render for Controls {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let switch_clicks = self.clicks.clone();
        let button_clicks = self.clicks.clone();
        v_flex()
            .size_full()
            .child(
                switch("editable", false, false, self.palette)
                    .on_click(move |_, _, _| switch_clicks.borrow_mut().push("switch")),
            )
            .child(switch("locked", true, true, self.palette))
            .child(disabled_button("disabled", "Saving…", self.palette))
            .child(
                button("save", "Save", self.palette).on_click(move |_, _, _| button_clicks.borrow_mut().push("save")),
            )
    }
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
}

fn press(cx: &mut TestAppContext, window: AnyWindowHandle, key: &str) {
    let os = cx.read(|cx| cx.platform_info().os);
    let keystroke = Keystroke::parse_with_os(key, os).unwrap();
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(KeyUpEvent { keystroke }.to_platform_input(), cx);
    })
    .unwrap();
}

#[test]
fn native_switch_and_button_support_keyboard_and_skip_disabled_controls() {
    for appearance in [WindowAppearance::Light, WindowAppearance::Dark] {
        let mut cx = TestAppContext::single();
        let clicks = Rc::new(RefCell::new(Vec::new()));
        let window = cx.add_window({
            let clicks = clicks.clone();
            move |_, _| Controls {
                clicks,
                palette: Palette::for_choice(crate::palette::ThemeChoice::System, appearance),
            }
        });
        draw(&mut cx, window.into());
        cx.update_window(window.into(), |_, window, cx| window.focus_next(cx))
            .unwrap();
        draw(&mut cx, window.into());
        press(&mut cx, window.into(), "space");
        assert_eq!(*clicks.borrow(), ["switch"]);
        cx.update_window(window.into(), |_, window, cx| window.focus_next(cx))
            .unwrap();
        draw(&mut cx, window.into());
        press(&mut cx, window.into(), "enter");
        assert_eq!(*clicks.borrow(), ["switch", "save"]);
        cx.update_window(window.into(), |_, window, cx| window.focus_prev(cx))
            .unwrap();
        draw(&mut cx, window.into());
        press(&mut cx, window.into(), "enter");
        assert_eq!(*clicks.borrow(), ["switch", "save", "switch"]);
    }
}

/// A reported bug: Settings › Connection drew the add field's address larger than the gateway rows. The
/// `Input` set its own 14 px and ignored the box's 13 px.
#[gpui_kit::test]
fn a_settings_field_draws_its_text_at_the_pages_13_px(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_kit::init(cx));
    let cx = cx.add_empty_window();
    cx.update(|window, cx| {
        let state = cx.new(|cx| InputState::new(window, cx));
        let mut input = input_text(&state);
        assert_eq!(input.style().text.font_size, Some(px(13.).into()));
    });
}

struct Details {
    width: f32,
    palette: Palette,
}

impl Render for Details {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(self.width)).font_family("DM Sans").child(
            setting_detail(
                "This setting controls how long Nexus keeps your conversations. Older conversations are deleted after the selected retention period. Your organization can enforce a shorter retention period. Review the complete rule before you save a change because deleted conversations cannot be recovered.",
                self.palette,
            ).debug_selector(|| "setting-explanation".into()),
        )
    }
}

#[gpui_kit::test]
fn setting_rules_remain_complete_at_narrow_widths(cx: &mut TestAppContext) {
    let (details, cx) = cx.add_window_view(|_, _| Details {
        width: 240.,
        palette: Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark),
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let narrow = cx.debug_bounds("setting-explanation").expect("explanation is drawn");
    assert!(
        narrow.size.height > px(51.),
        "a full rule must exceed the old three-line clamp"
    );
    assert_eq!(narrow.size.width, px(240.));
    details.update(cx, |view, cx| {
        view.width = 720.;
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let wide = cx.debug_bounds("setting-explanation").unwrap();
    assert!(wide.size.height < narrow.size.height);
    assert_eq!(wide.size.width, px(720.));
}

struct NotificationRow {
    width: f32,
    palette: Palette,
}

impl Render for NotificationRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(self.width)).font_family("DM Sans").child(
            card_row()
                .child(
                    setting_lines(
                        "Automations and integrations, including every provider key and every connector",
                        "",
                        self.palette,
                    )
                    .flex_1()
                    .debug_selector(|| "notification-label".into()),
                )
                .child(switch("notification-switch", true, false, self.palette)),
        )
    }
}

/// A Notifications row's label wraps beside its switch rather than truncating, so no text is lost and
/// no tooltip is needed to read it.
#[gpui_kit::test]
fn notification_labels_wrap_instead_of_truncating(cx: &mut TestAppContext) {
    let (row, cx) = cx.add_window_view(|_, _| NotificationRow {
        width: 240.,
        palette: Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark),
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let narrow = cx.debug_bounds("notification-label").expect("label is drawn");
    row.update(cx, |view, cx| {
        view.width = 900.;
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let wide = cx.debug_bounds("notification-label").unwrap();
    assert!(
        narrow.size.height > wide.size.height,
        "a long label must wrap onto more lines when narrow, not clip to one: {narrow:?} vs {wide:?}"
    );
}
