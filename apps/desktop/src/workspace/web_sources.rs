//! The web pages an answer's searches read. A chip in the
//! answer's footer shows up to three site tiles, "N sources" and a chevron. It is a disclosure, like a tool row: a
//! click opens the list in place, under the footer, and pushes the rest of the chat down; a second click closes it.
//! Each row shows the page's tile, title, site and date, and the query that found it on hover. A row with an http or
//! https link opens it.
//!
//! Whether the list is open is the answer's own (`Entry::sources_open`), kept while the chat is open. It is in the
//! entry's height key, so the chat's list measures that message again when it opens or closes.
//!
//! The tiles are the site's first letter (`WebSource::tile`), not its icon: an icon would make the app fetch from each
//! site, and so tell it who reads the answer.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use nexus_desktop_core::chat::{WebSource, source_tiles, sources_label};

use super::Workspace;
use crate::motion::{self, TweenExt as _};
use crate::palette::Palette;

/// How many site tiles the chip shows.
pub(super) const MAX_TILES: usize = 3;
/// The chip's height: the footer's 24 line.
const CHIP: f32 = 24.;
/// A site tile's side, and how far each tile tucks under the one before it.
const TILE: f32 = 16.;
const TUCK: f32 = 4.;

/// A site's tile: its letter on the raised fill, with an edge that parts it from the tile it covers.
fn tile(letter: char, palette: Palette) -> Div {
    div()
        .flex_none()
        .size(px(TILE))
        .rounded(px(4.))
        .bg(palette.raised)
        .border_1()
        .border_color(palette.border)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .line_height(px(TILE))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.foreground)
        .child(letter.to_string())
}

impl Workspace {
    /// Opens or closes answer `entry`'s sources list.
    fn toggle_sources(&mut self, entry: usize, cx: &mut Context<Self>) {
        if let Some(answer) = self.entries.iter_mut().find(|answer| answer.id == entry) {
            answer.sources_open = !answer.sources_open;
            cx.notify();
        }
    }
}

/// The chip in answer `entry`'s footer: the tiles, "N sources" and the chevron, which turns up while the list is open.
pub(super) fn chip(
    entry: usize,
    sources: &[WebSource],
    open: bool,
    palette: Palette,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let label = sources_label(sources.len());
    let color = if open { palette.foreground } else { palette.muted };
    h_flex()
        .id(("web-sources-chip", entry))
        .relative()
        .flex_none()
        .h(px(CHIP))
        .pl(px(4.))
        .pr(px(6.))
        .gap(px(8.))
        .rounded_full()
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(color)
        .cursor_pointer()
        .role(Role::Button)
        .aria_label(SharedString::from(format!(
            "{label} from the web, {}",
            if open { "open" } else { "closed" }
        )))
        .debug_selector(|| "web-sources-chip".into())
        .hover(|style| style.text_color(palette.foreground))
        .child(motion::hover_fill(
            ("web-sources-fill", entry),
            palette.hover,
            CHIP / 2.,
        ))
        .child(
            h_flex().children(
                source_tiles(sources, MAX_TILES)
                    .into_iter()
                    .enumerate()
                    .map(|(ix, letter)| tile(letter, palette).when(ix > 0, |tile| tile.ml(px(-TUCK)))),
            ),
        )
        .child(label)
        .child(
            Icon::new(Lucide::ChevronDown)
                .size(px(14.))
                .text_color(color)
                .with_tween(
                    ("web-sources-chevron", entry),
                    if open { 1. } else { 0. },
                    motion::Tween::new(motion::COLLAPSE, motion::ease_out),
                    |icon, turn| icon.rotate(radians(std::f32::consts::PI * turn)),
                ),
        )
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_sources(entry, cx)))
        .into_any_element()
}

/// The open list: one card on the bubble fill, a row for each page. Its rows fade in as it opens.
pub(super) fn list(entry: usize, sources: &[WebSource], palette: Palette, cx: &mut Context<Workspace>) -> AnyElement {
    let card = v_flex()
        .w_full()
        .p(px(4.))
        .gap(px(1.))
        .rounded(px(10.))
        .bg(palette.bubble)
        .debug_selector(|| "web-sources-list".into())
        .children(
            sources
                .iter()
                .enumerate()
                .map(|(ix, source)| row(entry, ix, source, palette, cx)),
        );
    if cx.reduce_motion() {
        card.into_any_element()
    } else {
        motion::fade_in(card, ("web-sources-list", entry)).into_any_element()
    }
}

/// One page: its tile, its title, its site and date, and the open-link icon on hover.
fn row(entry: usize, ix: usize, source: &WebSource, palette: Palette, cx: &mut Context<Workspace>) -> Stateful<Div> {
    let href = source.href();
    let group = SharedString::from(format!("web-source-{entry}-{ix}"));
    let found = SharedString::from(format!("Found by searching: {}", source.query));
    h_flex()
        .id(("web-source", entry * 1000 + ix))
        .group(group.clone())
        .items_start()
        .gap(px(8.))
        .p(px(8.))
        .rounded(px(6.))
        .debug_selector(|| "web-source-row".into())
        .when(!source.query.is_empty(), |row| {
            row.tooltip(move |window, cx| Tooltip::new(found.clone()).build(window, cx))
        })
        .child(div().pt(px(1.)).child(tile(source.tile(), palette)))
        .child(
            v_flex()
                .flex_1()
                .min_w(px(0.))
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.))
                        .line_height(px(18.))
                        .text_color(if href.is_some() {
                            palette.foreground
                        } else {
                            palette.muted
                        })
                        .debug_selector(|| "web-source-title".into())
                        .child(source.label()),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(palette.faint)
                        .debug_selector(|| "web-source-facts".into())
                        .child(source.facts()),
                ),
        )
        .when_some(href, |row, href| {
            row.cursor_pointer()
                .hover(|style| style.bg(palette.hover))
                .child(
                    div()
                        .flex_none()
                        .pt(px(2.))
                        .opacity(0.)
                        .group_hover(group, |style| style.opacity(1.))
                        .child(Icon::new(Lucide::ExternalLink).size(px(14.)).text_color(palette.icon)),
                )
                .on_click(cx.listener(move |_, _, _, cx| crate::external_link::open(&href, cx)))
        })
}

#[cfg(test)]
#[path = "web_sources_tests.rs"]
mod tests;
