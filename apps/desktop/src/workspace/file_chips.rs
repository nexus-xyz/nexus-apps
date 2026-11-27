//! A file an answer names, drawn as a chip in a Code session's text, as the Nexus app's `output-links.tsx`.
//!
//! A code span or Markdown link that names a file (`nexus_workbench::file_refs`) becomes a chip: the file type's icon,
//! as the Files tree shows it (`file_icons`), the file's name and its line. A click opens it in the Workbench's Files
//! tool at the line, or, for a format Files cannot show, in the system app; an arrow on the chip says so. Ctrl-click
//! (Cmd on a Mac) opens it in the person's editor, as a tool row does. A right click opens a menu (`link_menu.rs`).
//! Resting the pointer on it previews the file (`file_peek.rs`).
//!
//! The chip is a text view plugin: the text view parses it with the answer, off the UI thread, and draws it inline.

use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::HoverCard;
use gpui_kit::base::text::{
    InlineElement, InlineRenderContext, MarkdownNode, MarkdownParseContext, MarkdownPlugin, markdown_ast as mdast,
};
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_workbench::file_refs::{self, Opens};
use nexus_workbench::inline_media::FileReference;

use crate::file_icons::FileIcon;
use crate::palette::Palette;

/// The plugin's node name.
const NODE: &str = "nexus-file";

/// What a click on a chip asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ChipClick {
    /// Files, or the system app for a format Files cannot show.
    Open,
    /// The person's editor, at the line.
    Editor,
    /// The chip's menu, at this point of the window.
    Menu(Point<Pixels>),
}

pub(super) type ChipHandler = Arc<dyn Fn(&FileReference, ChipClick, &mut Window, &mut App) + Send + Sync>;

/// Where the session's files are read, and the paths its tool calls touched (`file_open::touched_files`), so the
/// preview finds the file a click opens. Asked for only when a chip's preview opens (`file_peek.rs`).
pub(super) type PlaceSource = Arc<dyn Fn(&App) -> Option<(crate::side::Place, Vec<String>)> + Send + Sync>;

/// A chip's file, parsed with the answer.
struct Chip {
    reference: FileReference,
    /// A link's own text; None for a code span, which shows the file's name.
    label: Option<String>,
}

/// The plugin a Code session's text view takes, so the files its answers name draw as chips.
#[derive(Clone)]
pub(super) struct FileChips {
    handler: ChipHandler,
    place: PlaceSource,
    palette: Palette,
    /// The session runs on this computer: the system app can open its files.
    local: bool,
}

impl FileChips {
    pub(super) fn new(handler: ChipHandler, place: PlaceSource, palette: Palette, local: bool) -> Self {
        FileChips {
            handler,
            place,
            palette,
            local,
        }
    }
}

impl MarkdownPlugin for FileChips {
    fn name(&self) -> &str {
        NODE
    }

    fn parse(&self, node: &mdast::Node, cx: &MarkdownParseContext<'_>) -> Option<MarkdownNode> {
        let (chip, text) = match node {
            mdast::Node::InlineCode(code) => (
                Chip {
                    reference: file_refs::code_reference(&code.value)?,
                    label: None,
                },
                code.value.clone(),
            ),
            mdast::Node::Link(link) => {
                let reference = file_refs::link_reference(&link.url)?;
                let label = plain_text(&link.children);
                let text = if label.is_empty() {
                    reference.path.clone()
                } else {
                    label.clone()
                };
                (
                    Chip {
                        reference,
                        label: (!label.is_empty()).then_some(label),
                    },
                    text,
                )
            }
            _ => return None,
        };
        let parsed = MarkdownNode::new(NODE, chip).text(text);
        // Copying the answer as Markdown keeps the span or link as written.
        Some(match cx.node_source(node) {
            Some(source) => parsed.markdown(source),
            None => parsed,
        })
    }

    fn render_inline(
        &self,
        node: &MarkdownNode,
        context: &InlineRenderContext,
        _: &mut Window,
        _: &mut App,
    ) -> Option<InlineElement> {
        let chip = node.data::<Chip>()?;
        let at = node.source_range().map_or(0, |range| range.start);
        let trigger = render_chip(chip, at, context, self.palette, self.local, self.handler.clone());
        // The preview: built when the card opens, never while nothing is hovered.
        let (reference, place, palette) = (chip.reference.clone(), self.place.clone(), self.palette);
        let card = HoverCard::new(("file-peek", at))
            .anchor(Anchor::TopLeft)
            .trigger(trigger)
            .content(move |_, window, cx| {
                // The card calls this on every frame while it is open. The preview is kept across those frames, so
                // its read finishes; it goes when the card closes, and the next open reads the file again.
                let key = SharedString::from(format!("file-peek:{at}:{}", reference.path));
                let peek = window.use_keyed_state(key, cx, move |_, cx| {
                    super::file_peek::FilePeek::new(reference, place(cx), palette, cx)
                });
                div().id("file-peek").child(peek)
            });
        Some(InlineElement::new(card))
    }
}

/// A link's text, without its marks: `[**the** notes](a.md)` is "the notes".
fn plain_text(nodes: &[mdast::Node]) -> String {
    let mut text = String::new();
    for node in nodes {
        match node {
            mdast::Node::Text(part) => text.push_str(&part.value),
            mdast::Node::InlineCode(part) => text.push_str(&part.value),
            other => {
                if let Some(children) = other.children() {
                    text.push_str(&plain_text(children));
                }
            }
        }
    }
    text.trim().to_string()
}

fn render_chip(
    chip: &Chip,
    at: usize,
    context: &InlineRenderContext,
    palette: Palette,
    local: bool,
    handler: ChipHandler,
) -> impl IntoElement + use<> {
    let reference = chip.reference.clone();
    let app = file_refs::opens(&reference.path, local) == Opens::App;
    let name = chip
        .label
        .clone()
        .unwrap_or_else(|| file_refs::file_name(&reference.path).to_string());
    let line = reference.line.filter(|_| chip.label.is_none());
    // Inline code's look (`transcript::prose_running`), and a hover that says it opens.
    let size = context.font_size() * 0.875;
    let (open, menu) = (handler.clone(), handler);
    let (clicked, pressed) = (reference.clone(), reference);
    h_flex()
        .id(("file-chip", at))
        .px(px(5.))
        .gap(px(4.))
        .rounded(px(4.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.sidebar)
        .font_family(crate::pages::ui::MONO)
        .text_size(size)
        .text_color(palette.strong)
        .cursor_pointer()
        .hover(|style| style.bg(palette.hover))
        // The file type's icon in its own colors, as the Files tree and the answer's file row show it.
        .child(
            img(FileIcon::file(&clicked.path).asset(palette.dark))
                .flex_none()
                .size(size),
        )
        .child(div().whitespace_nowrap().child(name))
        .when_some(line, |chip, line| {
            chip.child(div().text_color(palette.faint).child(format!(":{line}")))
        })
        .when(app, |chip| {
            chip.child(
                Icon::new(Lucide::ExternalLink)
                    .size(size * 0.9)
                    .text_color(palette.faint),
            )
        })
        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
            cx.stop_propagation();
            menu(&pressed, ChipClick::Menu(event.position), window, cx);
        })
        .on_click(move |event, window, cx| {
            if matches!(event, ClickEvent::Mouse(click) if click.up.button != MouseButton::Left) {
                return;
            }
            cx.stop_propagation();
            let click = if event.modifiers().secondary(cx) {
                ChipClick::Editor
            } else {
                ChipClick::Open
            };
            open(&clicked, click, window, cx);
        })
}

#[cfg(test)]
mod tests {
    use super::plain_text;
    use gpui_kit::base::text::markdown_ast as mdast;

    #[test]
    fn a_links_label_is_its_text_without_marks() {
        let nodes = vec![
            mdast::Node::Strong(mdast::Strong {
                children: vec![mdast::Node::Text(mdast::Text {
                    value: "the".into(),
                    position: None,
                })],
                position: None,
            }),
            mdast::Node::Text(mdast::Text {
                value: " notes ".into(),
                position: None,
            }),
        ];
        assert_eq!(plain_text(&nodes), "the notes");
    }
}
