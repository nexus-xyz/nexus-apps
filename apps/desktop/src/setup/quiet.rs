//! The two small parts a note uses around a setup line: the link at its end that goes to the fix ("Pick a project ›"),
//! and the quiet line for what nobody in the app can fix ("This Gateway can't share sessions yet.").

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{FixKey, Fixer, SetupLine, Tone};
use crate::palette::Palette;

/// The link at the end of a note: its label is the action, and ` ›` says it goes somewhere. The caller adds the click.
pub fn link(id: impl Into<ElementId>, label: &str, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .text_color(palette.foreground)
        .cursor_pointer()
        .hover(|style| style.underline())
        .child(format!("{label} ›"))
}

/// A line nobody in the app can fix: its icon (info, or a lock for a managed setting) and quiet text, no button.
pub fn info_line(
    key: impl Into<FixKey>,
    icon: Lucide,
    what: impl Into<SharedString>,
    palette: Palette,
    cx: &App,
) -> Stateful<Div> {
    // An info line runs no fix, so its fixer does nothing.
    let fixer = Fixer::new(None, |_, _, _, _| {});
    SetupLine::new(key, icon, what)
        .tone(Tone::Info)
        .render(&fixer, palette, cx)
}
