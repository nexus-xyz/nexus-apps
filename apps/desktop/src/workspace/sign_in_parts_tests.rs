use super::*;
use core::prelude::v1::test;

/// The sign-in column (`sign_in.rs`): 360 px wide, its parts centred, holding the keychain's long sentence.
struct Column;

impl Render for Column {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::for_choice(crate::palette::ThemeChoice::Dark, WindowAppearance::Dark);
        let sentence = "The keychain did not let Nexus use the sign-in from the old Nexus app. \
                        Nexus will not ask for it again. Sign in to continue.";
        div().size_full().child(
            v_flex()
                .debug_selector(|| "sign-in-column".into())
                .w(px(360.))
                .items_center()
                .child(error_line(sentence.into(), palette)),
        )
    }
}

#[gpui_kit::test]
fn a_long_error_wraps_inside_the_column(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (_view, cx) = cx.add_window_view(|_, _| Column);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let column = cx.debug_bounds("sign-in-column").unwrap();
    let text = cx.debug_bounds("sign-in-error-text").unwrap();
    assert!(
        text.right() <= column.right(),
        "the error ran past the column: {text:?} in {column:?}"
    );
    assert!(text.size.height > px(20.), "the error did not wrap: {text:?}");
}
