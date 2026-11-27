//! The tools of a Code session's side pane (`workspace/side_pane.rs` holds the pane): Changes, Files, Terminal and
//! Browser, each a view of the session's folder that works whether this build or the Nexus app runs the session.

pub mod browser;
pub mod changes;
pub(crate) mod file_lifetime;
pub mod file_view;
pub mod files;
pub mod pdf;
pub mod server;
pub mod shared_terminals;
pub mod terminal;
pub mod terminals;
pub(crate) mod well;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::code::Block;
use crate::palette::Palette;

/// How far a tool's content sits from the pane's edges: its first tab's icon, a terminal's text, the address field.
pub const INSET: f32 = 12.;

/// Where Changes and Files read: a folder on this computer, or a server's session or project (`server.rs`).
#[derive(Clone)]
pub enum Place {
    Here(Result<crate::code::SessionFolder, String>),
    Server(server::ServerPlace),
}

/// A tool's message where its content would be: an empty state or an error, centred in the tool as macOS and Linear
/// place theirs.
pub fn note(text: impl Into<SharedString>, palette: Palette) -> Div {
    empty_state(None, None, text, palette)
}

/// A tool's error where its content would be, placed as `note` places a message. A person can select the text and
/// copy it (Ctrl+C, Cmd+C on macOS, or the window's right-click Copy) into a bug report or a message to the agent.
/// The copy keys reach the selection because a press here gives the focus to the workspace, which tracks it, and
/// takes it from a text field (the composer, the commit message) that would take Ctrl+C itself.
pub fn error_note(id: impl Into<ElementId>, text: impl Into<SharedString>, palette: Palette) -> Div {
    centred().child(
        detail(palette)
            .debug_selector(|| "error-note".into())
            // Not centred: GPUI hit-tests a text's lines as if they start at its left edge, so a drag over a centred
            // line would select other characters than the ones under the pointer.
            .child(crate::workspace::message_text::MessageText::new(id, text.into())),
    )
}

/// A centred empty state: an optional 28px icon, an optional 14px title, and a muted line under them.
pub fn empty_state(
    icon: Option<Lucide>,
    title: Option<&'static str>,
    detail_text: impl Into<SharedString>,
    palette: Palette,
) -> Div {
    centred()
        .when_some(icon, |state, icon| {
            state.child(Icon::new(icon).size(px(28.)).text_color(palette.faint).mb(px(6.)))
        })
        .when_some(title, |state, title| {
            state.child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.foreground)
                    .child(title),
            )
        })
        .child(detail(palette).text_center().child(detail_text.into()))
}

/// The column an empty state or an error sits in, centred in the tool.
fn centred() -> Div {
    v_flex()
        .flex_1()
        .min_h(px(0.))
        .size_full()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .px(px(32.))
        .pb(px(40.))
}

/// The muted 13px line under an empty state's title, or an error.
fn detail(palette: Palette) -> Div {
    div()
        .max_w(px(320.))
        .text_size(px(13.))
        .line_height(px(19.))
        .text_color(palette.muted)
}

/// A text field's fill on the pane: white on light grey, a step up from the pane in dark. No ring: the fill is the
/// edge.
pub fn field_fill(palette: Palette) -> Hsla {
    if palette.dark { palette.selected } else { palette.raised }
}

/// A 26px icon button in a tool's own bar.
pub fn icon_button(id: impl Into<ElementId>, icon: Lucide, palette: Palette) -> Stateful<Div> {
    icon_button_when(id, icon, true, palette)
}

/// A 26px icon button that may be off: then `faint`, with no hover, and the caller leaves out its click.
pub fn icon_button_when(id: impl Into<ElementId>, icon: Lucide, enabled: bool, palette: Palette) -> Stateful<Div> {
    let button = div()
        .id(id)
        .size(px(26.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.));
    if enabled {
        button
            .cursor_pointer()
            .hover(|style| style.bg(palette.hover))
            .active(|style| style.opacity(0.8))
            .child(Icon::new(icon).size(px(14.)).text_color(palette.icon))
    } else {
        button.child(Icon::new(icon).size(px(14.)).text_color(palette.faint.opacity(0.55)))
    }
}

/// A choice among a few, as a text chip: filled while chosen.
pub fn chip(id: impl Into<ElementId>, label: &'static str, chosen: bool, palette: Palette) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(26.))
        .px(px(9.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .text_size(px(12.5))
        .cursor_pointer()
        .when(chosen, |chip| chip.bg(palette.selected).text_color(palette.foreground))
        .when(!chosen, |chip| {
            chip.text_color(palette.muted)
                .hover(|style| style.bg(palette.hover).text_color(palette.foreground))
        })
        .child(label)
}

/// The last local address (`http://localhost:3000`) the session's agent wrote or ran into: likely its dev server.
pub fn latest_local_url(blocks: &[Block]) -> Option<String> {
    blocks.iter().rev().find_map(|block| {
        let text = match block {
            Block::Text(text) => text.as_str(),
            Block::Tool { summary, .. } => summary.as_str(),
            _ => return None,
        };
        local_urls(text).last().cloned()
    })
}

/// Every local address the session's agent wrote or ran into, newest first, each once: its dev servers.
pub fn session_urls(blocks: &[Block]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for block in blocks.iter().rev() {
        let text = match block {
            Block::Text(text) => text.as_str(),
            Block::Tool { summary, .. } => summary.as_str(),
            _ => continue,
        };
        for url in local_urls(text).into_iter().rev() {
            if !found.contains(&url) {
                found.push(url);
            }
        }
    }
    found
}

fn local_urls(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for start in ["http://localhost", "http://127.0.0.1", "http://0.0.0.0"] {
        let mut rest = text;
        while let Some(at) = rest.find(start) {
            let url: String = rest[at..]
                .chars()
                .take_while(|c| !c.is_whitespace() && !matches!(c, '`' | '"' | '\'' | ')' | '>' | ']' | ','))
                .collect();
            let url = url.trim_end_matches(['.', ':']).replace("0.0.0.0", "localhost");
            found.push((at + text.len() - rest.len(), url));
            rest = &rest[at + start.len()..];
        }
    }
    found.sort();
    found.into_iter().map(|(_, url)| url).collect()
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::*;

    #[test]
    fn the_latest_local_address_is_the_dev_server() {
        let blocks = vec![
            Block::Text("Started at http://localhost:3000.".into()),
            Block::Text("Now `http://127.0.0.1:5173/app` and nothing else".into()),
            Block::User {
                seq: 1,
                at: 0,
                text: "open http://localhost:9999".into(),
                from: None,
                rewound: false,
                images: 0,
                image_data: Vec::new(),
            },
        ];
        assert_eq!(latest_local_url(&blocks).as_deref(), Some("http://127.0.0.1:5173/app"));
        assert_eq!(latest_local_url(&blocks[..1]).as_deref(), Some("http://localhost:3000"));
        assert_eq!(latest_local_url(&[Block::Text("none".into())]), None);
        assert_eq!(
            session_urls(&blocks),
            ["http://127.0.0.1:5173/app", "http://localhost:3000"],
            "newest first, the person's messages left out"
        );
    }

    /// A text field over a tool's error, as the composer or the commit message sits beside the Changes tab, both in an
    /// element that tracks the focus, as the workspace does (`workspace/mod.rs`, its `track_focus`).
    struct ErrorProbe {
        field: Entity<gpui_kit::component::input::InputState>,
        focus: FocusHandle,
    }

    impl Render for ErrorProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let palette = Palette::for_choice(crate::palette::ThemeChoice::Light, WindowAppearance::Light);
            v_flex()
                .size_full()
                .track_focus(&self.focus)
                .child(
                    div()
                        .h(px(30.))
                        .child(gpui_kit::component::input::Input::new(&self.field)),
                )
                .child(error_note("probe-error", PROBE_ERROR, palette))
        }
    }

    const PROBE_ERROR: &str = "patch line 53: hunk ended before its declared line counts";

    #[gpui_kit::test]
    fn error_note_text_copies_with_the_copy_keys_while_a_field_had_focus(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(|window, cx| {
            let probe = cx.new(|cx| {
                let field = cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx));
                field.update(cx, |field, cx| field.focus(window, cx));
                ErrorProbe {
                    field,
                    focus: cx.focus_handle(),
                }
            });
            gpui_kit::base::Root::new(probe, window, cx)
        });
        let probe = window.update(cx, |root, _, _| root.view().clone()).unwrap();
        let probe = probe.downcast::<ErrorProbe>().unwrap();
        let cx = &mut VisualTestContext::from_window(window.into(), cx);
        let field_focused =
            |window: &Window, cx: &App| probe.read(cx).field.read(cx).focus_handle(cx).is_focused(window);
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear(cx);
            assert!(field_focused(window, cx), "the field starts with the focus");
        });
        let text = cx.debug_bounds("error-note").expect("the error is drawn");
        let (start, end) = (
            text.origin + point(px(1.), px(5.)),
            point(text.right() - px(1.), text.bottom() - px(5.)),
        );
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::none());
        cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none());
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(gpui_kit::base::TextSelection::selected_text(window, cx), PROBE_ERROR);
            assert!(!field_focused(window, cx), "the press took the focus from the field");
        });
        cx.simulate_keystrokes(if cfg!(target_os = "macos") { "cmd-c" } else { "ctrl-c" });
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
                Some(PROBE_ERROR)
            )
        });
    }
}
