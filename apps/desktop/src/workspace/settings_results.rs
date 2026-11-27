//! Settings search results, drawn in the frame every settings section uses: the same gutter, the same centred form
//! column and the same title row. The title then stays where it was when a query starts or ends.
//!
//! Hits keep their rank. A head names the nav row each time it changes, so ↑ and ↓ still walk the list in order.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::settings_registry::Dest;
use super::settings_search::Hit;
use super::{View, Workspace};
use crate::pages::ui;
use crate::palette::Palette;

/// Below this window width the page's margins narrow.
pub(super) const NARROW_WIDTH: f32 = 900.;

/// The page's side padding: the settings gutter, or 24px in a narrow window.
pub(super) fn settings_gutter(narrow: bool) -> Pixels {
    px(if narrow { 24. } else { ui::SETTINGS_GUTTER })
}

/// A settings page's title, the one large heading on it. Sections and search draw it the same way.
pub(super) fn page_title(title: impl Into<SharedString>, palette: Palette) -> Div {
    div()
        .debug_selector(|| "settings-page-title".into())
        .flex_1()
        .min_w(px(0.))
        .truncate()
        .text_size(px(20.))
        .line_height(px(28.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.strong)
        .child(title.into())
}

/// The head before each hit: its nav row when that differs from the hit before it, else None.
fn result_heads(hits: &[Hit]) -> Vec<Option<&'static str>> {
    let mut last = None;
    hits.iter()
        .map(|hit| {
            let head = hit.breadcrumb.first().copied();
            let changed = head != last;
            last = head;
            head.filter(|_| changed)
        })
        .collect()
}

/// The breadcrumb between the head and the hit's own label: "Servers" for "This computer › Servers › Sign-ins".
fn result_path(hit: &Hit) -> Option<String> {
    let crumbs = hit.breadcrumb.get(1..).unwrap_or_default();
    let crumbs = match crumbs.split_last() {
        Some((last, rest)) if *last == hit.label => rest,
        _ => crumbs,
    };
    (!crumbs.is_empty()).then(|| crumbs.join(" › "))
}

/// What a row says at its end: how Enter opens the selected row, and that a hit this build does not draw opens in
/// the browser.
fn result_hint(web: bool, selected: bool) -> Option<&'static str> {
    match (web, selected) {
        (true, true) => Some("↵ Open in browser"),
        (true, false) => Some("Opens in browser"),
        (false, true) => Some("↵ Open"),
        (false, false) => None,
    }
}

impl Workspace {
    /// The search view, in place of the open section while a query is typed.
    pub(super) fn render_settings_results(
        &self,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let hits = self.settings_hits(cx);
        let count = hits.len();
        let selected = self.settings.shell.selected.min(count.saturating_sub(1));
        let narrow = window.viewport_size().width / px(1.) < NARROW_WIDTH;
        let heads = result_heads(&hits);
        let rows: Vec<AnyElement> = hits
            .into_iter()
            .enumerate()
            .flat_map(|(ix, hit)| {
                let head = heads[ix].map(|text| {
                    ui::section_head(text, palette)
                        .when(ix == 0, |head| head.mt(px(0.)))
                        .into_any_element()
                });
                let path = result_path(&hit);
                let locked = self.is_local() && hit.requires_sign_in();
                let web = hit.section.is_none() && !locked;
                let anchor = ScrollAnchor::for_handle(self.settings.shell.results_scroll.clone());
                if selected == ix && self.settings.shell.results_selected.replace(Some(selected)) != Some(selected) {
                    let window = window.window_handle();
                    let reveal = anchor.clone();
                    let epoch = self.navigation_epoch;
                    let at = self.history_at;
                    let query = self.settings.shell.query.read(cx).value().to_string();
                    let weak = cx.weak_entity();
                    cx.defer(move |cx| {
                        let _ = window.update(cx, |_, window, cx| {
                            let current = weak.upgrade().is_some_and(|view| {
                                view.read_with(cx, |view, cx| {
                                    view.navigation_epoch == epoch
                                        && view.history_at == at
                                        && view.navigation_allowed()
                                        && matches!(view.view, View::Settings(_))
                                        && view.settings.shell.searching
                                        && view.settings.shell.selected == selected
                                        && view.settings.shell.query.read(cx).value().as_ref() == query
                                })
                            });
                            if current {
                                reveal.scroll_to(window, cx);
                            }
                        });
                    });
                }
                let is_selected = selected == ix;
                let label = h_flex()
                    .min_w(px(0.))
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(13.))
                            .text_color(palette.strong)
                            .child(hit.label),
                    )
                    .when_some(path, |line, path| {
                        line.child(
                            div()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                .child(path),
                        )
                    })
                    .when(locked, |line| line.child(ui::requires_sign_in(palette)));
                let row = ui::row(palette)
                    .id(SharedString::from(format!("settings-result-{ix}")))
                    .debug_selector(move || format!("settings-result-{ix}"))
                    .anchor_scroll(Some(anchor))
                    .items_start()
                    .cursor_pointer()
                    .when(is_selected, |row| row.bg(palette.selected))
                    .when(!is_selected, |row| row.hover(|style| style.bg(palette.hover)))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(2.))
                            .child(label)
                            .child(ui::setting_detail(hit.description, palette)),
                    )
                    .children(result_hint(web, is_selected).map(|hint| {
                        div()
                            .flex_none()
                            .pt(px(1.))
                            .text_size(px(12.))
                            .text_color(palette.faint)
                            .child(hint)
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.settings.shell.selected = ix;
                        this.open_hit(&hit, window, cx);
                    }));
                head.into_iter().chain(std::iter::once(row.into_any_element()))
            })
            .collect();
        let header = h_flex()
            .h(px(28.))
            .gap(px(12.))
            .child(page_title("Search settings", palette))
            .child(
                div()
                    .flex_none()
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child(format!("{count} {}", if count == 1 { "result" } else { "results" })),
            );
        let body = v_flex()
            .w_full()
            .min_w(px(0.))
            .pt(px(24.))
            .when(count == 0, |body| {
                body.child(ui::empty(
                    "No matching settings",
                    "Try another term, or browse This computer.",
                    palette,
                ))
                .child(
                    h_flex().justify_center().child(
                        ui::button("settings-empty-computer", "Browse This computer", palette)
                            .debug_selector(|| "settings-empty-computer".into())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.settings
                                    .shell
                                    .query
                                    .update(cx, |state, cx| state.set_value("", window, cx));
                                this.settings.shell.query_text.clear();
                                this.settings.shell.searching = false;
                                this.open_dest(Dest::Computer, window, cx);
                            })),
                    ),
                )
            })
            .children(rows)
            .when(count > 0, |body| {
                body.child(ui::setting_detail("↑ ↓ to move · Enter to open · Esc to clear", palette).mt(px(16.)))
            });
        div()
            .id("settings-results")
            .track_scroll(&self.settings.shell.results_scroll)
            .size_full()
            .overflow_y_scroll()
            .px(settings_gutter(narrow))
            .pb(px(64.))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(ui::SETTINGS_COLUMN))
                    .mx_auto()
                    .child(header)
                    .child(body),
            )
    }
}

#[cfg(test)]
#[path = "settings_results_tests.rs"]
mod tests;
