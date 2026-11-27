//! The Library's gallery and its switch, as Nexus UI's (`library/gallery.tsx`, `cover.tsx`, `ViewSwitch` in
//! `library.tsx`). A card is a 16:10 cover in its owner's color over its title, owner, age, share count and
//! kind. A page's cover is its kind's mark and its title; a chat's is its title as a question above an answer's
//! outline. A click opens the item, as a row of the list does; the web selects first because it has a preview pane.
//! Not here yet: page posters (`GET /v1/pages/{id}/versions/{version}/poster`), the preview pane and arrow keys.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::menu::ContextMenuExt;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::super::{Workspace, glyph, initials};
use super::{kind_icon, pill};
use crate::gateway::Account;
use crate::library::layout::{self, CARD_GAP, Layout};
use crate::library::{self, Kind, Open, Row};
use crate::palette::Palette;

/// A card's padding around its picture and text, as `.library-card`.
const PAD: f32 = 8.;

impl Workspace {
    /// Gallery or list: two marks in a track, the picked one filled, left of the search.
    pub(super) fn layout_switch(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.library.layout;
        h_flex()
            .flex_none()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(8.))
            .bg(palette.hover)
            .children(Layout::ALL.map(|each| {
                let label = each.label();
                let icon = match each {
                    Layout::Gallery => Lucide::LayoutGrid,
                    Layout::List => Lucide::List,
                };
                let id = format!("library-layout-{}", label.to_lowercase());
                h_flex()
                    .id(SharedString::from(id.clone()))
                    .debug_selector(move || id.clone())
                    .w(px(28.))
                    .h(px(26.))
                    .justify_center()
                    .rounded(px(6.))
                    .tab_index(0)
                    .cursor_pointer()
                    .focus_visible(move |style| style.shadow(vec![ring(palette)]))
                    .when(each == current, |button| button.bg(palette.selected))
                    .child(glyph(
                        icon,
                        if each == current { palette.strong } else { palette.muted },
                    ))
                    .aria_label(label)
                    .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_library_layout(each, cx)))
            }))
    }

    fn set_library_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        if self.library.layout == layout {
            return;
        }
        self.library.layout = layout;
        self.library.scroll.set_offset(point(px(0.), px(0.)));
        // A failed save keeps the choice for this run; the next launch shows the list.
        if let Err(error) = layout.save() {
            eprintln!("nexus-desktop-gpui: library layout: {error}");
        }
        cx.notify();
    }

    /// The cards of `rows` in lines as wide as `width`, as `.library-grid`.
    pub(super) fn render_cards(
        &self,
        rows: Vec<&Row>,
        width: f32,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let columns = layout::columns(width);
        let card = card_width(width, columns);
        let now = library::chrono_lite::now();
        let cards: Vec<AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(ix, row)| self.card(ix, row, card, now, palette, cx).into_any_element())
            .collect();
        lines(cards, columns).into_any_element()
    }

    fn card(
        &self,
        ix: usize,
        row: &Row,
        width: f32,
        now: library::chrono_lite::Seconds,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let opening = self.library_opening.as_deref() == Some(row.key.as_str());
        let page = matches!(row.open, Open::SharedPage(_) | Open::Page(_));
        let owner = owner(row, self.account.as_ref());
        let hue = route_hue(layout::hue_slot(&owner.key));
        let shares = row.shares.len();
        let open = row.clone();
        let context_row = row.clone();
        let workspace = cx.entity().downgrade();
        v_flex()
            .id(("library-card", ix))
            .debug_selector(move || format!("library-card-{ix}"))
            .flex_none()
            .w(px(width))
            .p(px(PAD))
            .rounded(px(16.))
            .tab_index(0)
            .cursor_pointer()
            .when(page, |card| {
                card.on_hover(|hovered, _, cx| crate::web::warm_on_hover(*hovered, cx))
            })
            .when(opening, |card| card.bg(palette.selected))
            .hover(|style| style.bg(palette.hover))
            .focus_visible(|style| style.bg(palette.hover))
            .active(|style| style.bg(palette.selected))
            .when(row.revoked, |card| card.opacity(0.55))
            .child(cover(row, width - 2. * PAD, hue, palette))
            .child(
                v_flex()
                    .gap(px(6.))
                    .pt(px(10.))
                    .px(px(2.))
                    .child(
                        h_flex()
                            .items_start()
                            .gap(px(6.))
                            .min_h(px(TITLE_LINE * 2.))
                            .when(row.revoked, |title| title.child(pill("Revoked", palette)))
                            .when(row.outside, |title| title.child(pill("Outside", palette)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_size(px(13.))
                                    .line_height(px(TITLE_LINE))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(palette.strong)
                                    .line_clamp(2)
                                    .child(row.title.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(px(6.))
                            .min_w(px(0.))
                            .text_size(px(12.))
                            .text_color(palette.muted)
                            .child(avatar(&owner, hue, palette))
                            .child(div().min_w(px(0.)).truncate().child(owner.name.clone()))
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(palette.faint)
                                    .child(format!("· {}", library::short_ago(&row.updated_at, now))),
                            )
                            .child(
                                h_flex()
                                    .flex_none()
                                    .ml_auto()
                                    .gap(px(6.))
                                    .when(shares > 1, |end| end.child(share_count(shares, palette)))
                                    .child(glyph(kind_icon(row), palette.muted))
                                    .child(self.library_more(row, cx)),
                            ),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !open.revoked {
                    this.open_row(&open, cx);
                }
            }))
            .context_menu(move |menu, _, cx| {
                let Some(workspace) = workspace.upgrade() else {
                    return menu;
                };
                workspace.update(cx, |this, cx| this.library_menu(menu, &context_row, cx))
            })
    }
}

/// A title line's height, as `.library-card-title`'s 13 px at 1.35.
const TITLE_LINE: f32 = 13. * 1.35;

/// A card's width when `columns` share `width`, gaps between them.
fn card_width(width: f32, columns: usize) -> f32 {
    ((width - CARD_GAP * (columns.saturating_sub(1)) as f32) / columns.max(1) as f32).max(0.)
}

/// `cards`, `columns` to a line.
fn lines(cards: Vec<AnyElement>, columns: usize) -> Div {
    let mut cards = cards.into_iter().peekable();
    let mut column = v_flex().gap(px(CARD_GAP));
    while cards.peek().is_some() {
        let line: Vec<AnyElement> = cards.by_ref().take(columns.max(1)).collect();
        column = column.child(h_flex().items_start().gap(px(CARD_GAP)).children(line));
    }
    column
}

/// Whose a row is, as `ownerOf` in `cover.tsx`: a share's author, else you. `key` picks the color.
struct Owner {
    name: String,
    initials: SharedString,
    key: String,
}

fn owner(row: &Row, account: Option<&Account>) -> Owner {
    if row.mine || matches!(row.open, Open::Page(_)) {
        let name = account.map(|account| account.name.as_str()).unwrap_or("You");
        let key = account
            .map(|account| account.email.clone())
            .filter(|email| !email.is_empty())
            .unwrap_or_else(|| "you".into());
        return Owner {
            name: row.shared_by.clone(),
            initials: initials(name),
            key,
        };
    }
    let key = if row.author_email.is_empty() {
        row.shared_by.clone()
    } else {
        row.author_email.clone()
    };
    Owner {
        name: row.shared_by.clone(),
        initials: initials(&row.shared_by),
        key: if key.is_empty() { "someone".into() } else { key },
    }
}

/// Nexus UI's route hues (`--nx-route-1`, 2, 4 and 5), the same in both themes.
fn route_hue(slot: u8) -> Hsla {
    match slot {
        1 => rgb(0x68CCF5).into(),
        2 => rgb(0xF1AD71).into(),
        4 => rgb(0xA9B8FF).into(),
        _ => rgb(0xDCA7EC).into(),
    }
}

/// The card's picture: 16:10, its sizes in parts of its width so it reads the same at any card width.
fn cover(row: &Row, width: f32, hue: Hsla, palette: Palette) -> Div {
    let height = width * 10. / 16.;
    let at = |part: f32| px(width * part);
    let picture = v_flex().size_full().px(at(0.05)).py(at(0.045)).bg(linear_gradient(
        155.,
        linear_color_stop(hue.opacity(0.32), 0.),
        linear_color_stop(hue.opacity(0.), 0.72),
    ));
    let picture = if row.artifact_kind() == Kind::Chat {
        picture
            .gap(at(0.04))
            .child(
                h_flex().justify_end().child(
                    div()
                        .max_w(at(0.8))
                        .px(at(0.035))
                        .py(at(0.025))
                        .rounded(at(0.04))
                        .bg(palette.raised.blend(hue.opacity(0.3)))
                        .text_size(at(0.046))
                        .line_height(at(0.046 * 1.3))
                        .text_color(palette.strong)
                        .line_clamp(3)
                        .child(row.title.clone()),
                ),
            )
            .child(v_flex().gap(at(0.025)).children([0.92, 0.84, 0.58].map(|part| {
                div()
                    .w(relative(part))
                    .h(at(0.022))
                    .rounded(at(0.011))
                    .bg(palette.strong.opacity(0.17))
            })))
    } else {
        picture
            .justify_between()
            .child(glyph(kind_icon(row), palette.strong.blend(hue.opacity(0.7))).size(at(0.07)))
            .child(
                div()
                    .max_w(relative(0.85))
                    .text_size(at(0.055))
                    .line_height(at(0.055 * 1.12))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.strong)
                    .line_clamp(3)
                    .child(row.title.clone()),
            )
    };
    div()
        .w(px(width))
        .h(px(height))
        .overflow_hidden()
        .rounded(px(10.))
        .border_1()
        .border_color(palette.border)
        .bg(palette.hover)
        .child(picture)
}

fn avatar(owner: &Owner, hue: Hsla, palette: Palette) -> Div {
    h_flex()
        .flex_none()
        .size(px(18.))
        .justify_center()
        .rounded_full()
        .bg(palette.background.blend(hue.opacity(0.3)))
        .text_size(px(8.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.strong)
        .child(owner.initials.clone())
}

/// "Shared N ways, one page".
fn share_count(shares: usize, palette: Palette) -> Div {
    h_flex()
        .gap(px(3.))
        .pl(px(5.))
        .pr(px(6.))
        .rounded_full()
        .bg(palette.hover)
        .text_size(px(11.))
        .child(glyph(Lucide::Link2, palette.muted).size(px(11.)))
        .child(shares.to_string())
}

fn ring(palette: Palette) -> BoxShadow {
    BoxShadow {
        color: palette.muted,
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(2.),
        inset: false,
    }
}

/// Eight grey cards the size of the gallery while the first read runs.
pub(super) fn skeleton(width: f32, palette: Palette) -> Div {
    let columns = layout::columns(width);
    let card = card_width(width, columns);
    let cards = (0..8)
        .map(|ix| {
            v_flex()
                .flex_none()
                .w(px(card))
                .p(px(PAD))
                .gap(px(8.))
                .child(
                    div()
                        .w_full()
                        .h(px((card - 2. * PAD) * 10. / 16.))
                        .rounded(px(10.))
                        .bg(palette.hover),
                )
                .child(
                    div()
                        .h(px(12.))
                        .w(relative(0.8 - (ix % 3) as f32 * 0.1))
                        .rounded(px(4.))
                        .bg(palette.hover),
                )
                .child(div().h(px(10.)).w(relative(0.45)).rounded(px(4.)).bg(palette.hover))
                .into_any_element()
        })
        .collect();
    lines(cards, columns)
}

#[cfg(test)]
mod tests {
    use super::card_width;

    #[test]
    fn cards_share_the_line() {
        assert_eq!(card_width(984., 4), (984. - 36.) / 4.);
        assert_eq!(card_width(300., 1), 300.);
        assert_eq!(card_width(0., 0), 0.);
    }
}
