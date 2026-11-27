//! Docs: Nexus UI's docs, drawn here from the Markdown the site serves (`docs.rs`). The sections of `/llms.txt` on
//! the left, the page on the right with the chat's Markdown. A link to another docs page opens it here; any other link
//! opens in the browser.

use crate::workspace::overlay::OverlaySlot;
use gpui_kit::component::text::TextViewState;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::transcript::prose;
use super::{Place, View, Workspace};
use crate::docs::{self, Section};
use crate::palette::Palette;

/// The navigation's width.
const NAV: f32 = 220.;
/// The page's widest, as the chat column.
const PAGE: f32 = 760.;

/// What the Docs view shows.
pub(super) struct DocsState {
    /// The navigation; `None` until `/llms.txt` comes back.
    pub sections: Option<Result<Vec<Section>, SharedString>>,
    /// The open page, by its path under `/docs` (empty for `/docs`).
    pub path: String,
    pub page: Option<Entity<TextViewState>>,
    pub loading: bool,
    pub error: Option<SharedString>,
    pub scroll: ScrollHandle,
}

impl DocsState {
    pub fn new() -> DocsState {
        DocsState {
            sections: None,
            path: String::new(),
            page: None,
            loading: false,
            error: None,
            scroll: ScrollHandle::new(),
        }
    }
}

impl Workspace {
    /// Open a docs page by its path under `/docs`.
    pub(super) fn open_docs(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let same = matches!(self.view, View::Docs) && self.docs.path == path && self.docs.page.is_some();
        self.visit(Place::Docs(path.clone()));
        if !matches!(self.view, View::Docs) {
            self.reset_chat();
            self.view = View::Docs;
        }
        self.overlay.close_person_menu();
        self.title = "Docs".into();
        if !matches!(self.docs.sections, Some(Ok(_))) {
            account_gateway.docs_index();
        }
        if !same {
            self.docs.path = path.clone();
            self.docs.loading = true;
            self.docs.error = None;
            account_gateway.docs_page(path);
        }
        cx.notify();
    }

    pub(super) fn docs_index_loaded(&mut self, sections: Result<Vec<Section>, String>) {
        match (sections, &self.docs.sections) {
            (Ok(sections), _) => self.docs.sections = Some(Ok(sections)),
            (Err(_), Some(Ok(_))) => {}
            (Err(error), _) => self.docs.sections = Some(Err(error.into())),
        }
    }

    pub(super) fn docs_page_loaded(&mut self, page: Result<(String, String), String>, cx: &mut Context<Self>) {
        match page {
            // Only the page last asked for.
            Ok((path, markdown)) if path == self.docs.path => {
                self.docs.page = Some(cx.new(|cx| TextViewState::markdown(markdown.as_str(), cx)));
                self.docs.loading = false;
                self.docs.error = None;
                self.docs.scroll.set_offset(point(px(0.), px(0.)));
            }
            Ok(_) => {}
            Err(error) => {
                self.docs.loading = false;
                self.docs.error = Some(error.into());
            }
        }
    }

    /// What a link on a docs page does: a docs page opens here, anything else in the browser.
    fn docs_link_opener(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&SharedString, &ClickEvent, &mut Window, &mut App) + Send + Sync + 'static {
        let this = cx.entity().downgrade();
        let ui = self.gateway.site_url();
        let current = self.docs.path.clone();
        move |href, _, _, cx| match resolve(&ui, &current, href) {
            Link::Docs(path) => {
                let _ = this.update(cx, |this, cx| this.open_docs(path, cx));
            }
            Link::Browser(url) => crate::external_link::open(&url, cx),
            Link::Anchor => {}
        }
    }

    pub(super) fn render_docs(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.docs;
        let nav: AnyElement = match &state.sections {
            None => v_flex()
                .gap(px(8.))
                .children((0..8).map(|ix| {
                    div()
                        .h(px(10.))
                        .w(px(120. + (ix % 3) as f32 * 30.))
                        .rounded(px(4.))
                        .bg(palette.hover)
                }))
                .into_any_element(),
            Some(Err(_)) => div()
                .text_size(px(12.))
                .text_color(palette.faint)
                .child("The contents didn't load.")
                .into_any_element(),
            Some(Ok(sections)) => v_flex()
                .gap(px(14.))
                .children(sections.iter().enumerate().map(|(section_ix, section)| {
                    v_flex()
                        .gap(px(1.))
                        .child(
                            div()
                                .px(px(8.))
                                .pb(px(4.))
                                .text_size(px(11.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette.faint)
                                .child(section.title.clone()),
                        )
                        .children(section.pages.iter().enumerate().map(|(page_ix, link)| {
                            let selected = link.path == state.path;
                            let path = link.path.clone();
                            div()
                                .id(SharedString::from(format!("docs-nav-{section_ix}-{page_ix}")))
                                .px(px(8.))
                                .py(px(4.))
                                .rounded(px(6.))
                                .text_size(px(13.))
                                .truncate()
                                .cursor_pointer()
                                .when(selected, |row| row.bg(palette.selected).text_color(palette.strong))
                                .when(!selected, |row| {
                                    row.text_color(palette.muted)
                                        .hover(|style| style.bg(palette.hover).text_color(palette.strong))
                                })
                                .child(link.title.clone())
                                .on_click(cx.listener(move |this, _, _, cx| this.open_docs(path.clone(), cx)))
                        }))
                }))
                .into_any_element(),
        };
        let page: AnyElement = match (&state.page, &state.error) {
            (_, Some(error)) if !state.loading => v_flex()
                .items_center()
                .py(px(64.))
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette.strong)
                        .child("This Artifact didn't load"),
                )
                .child(div().text_size(px(13.)).text_color(palette.faint).child(error.clone()))
                .child(
                    div()
                        .id("docs-retry")
                        .mt(px(10.))
                        .px(px(12.))
                        .py(px(5.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(palette.border)
                        .text_size(px(13.))
                        .cursor_pointer()
                        .hover(|style| style.bg(palette.hover))
                        .child("Try again")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let path = this.docs.path.clone();
                            this.docs.page = None;
                            this.open_docs(path, cx);
                        })),
                )
                .into_any_element(),
            (Some(page), _) => div()
                .w_full()
                .text_size(px(14.))
                .line_height(px(22.))
                .when(state.loading, |column| column.opacity(0.6))
                .child(prose(
                    crate::external_link::text_view(page).selectable(true).on_link_click({
                        let handler = self.context_link_handler(self.docs_link_opener(cx), cx);
                        move |url, event, window, cx| handler(url, event, window, cx)
                    }),
                    palette,
                ))
                .into_any_element(),
            (None, _) => v_flex()
                .gap(px(10.))
                .child(div().h(px(22.)).w(px(260.)).rounded(px(4.)).bg(palette.hover))
                .children((0..6).map(|ix| {
                    div()
                        .h(px(10.))
                        .w(relative(0.6 + (ix % 3) as f32 * 0.12))
                        .rounded(px(4.))
                        .bg(palette.hover)
                }))
                .into_any_element(),
        };
        h_flex()
            .size_full()
            .items_start()
            .child(
                div()
                    .id("docs-nav")
                    .w(px(NAV))
                    .h_full()
                    .flex_none()
                    .overflow_y_scroll()
                    .border_r_1()
                    .border_color(palette.border)
                    .px(px(12.))
                    .py(px(24.))
                    .child(nav),
            )
            .child(
                div()
                    .id("docs-page")
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .overflow_y_scroll()
                    .track_scroll(&state.scroll)
                    .child(
                        div()
                            .w_full()
                            .max_w(px(PAGE))
                            .mx_auto()
                            .px(px(32.))
                            .pt(px(28.))
                            .pb(px(48.))
                            .child(page),
                    ),
            )
    }
}

/// Where a link on a docs page leads.
#[derive(Debug, PartialEq)]
enum Link {
    /// A docs page, by its path under `/docs`.
    Docs(String),
    Browser(String),
    /// A heading on the same page.
    Anchor,
}

/// Read a link on the docs page at `current`: absolute and site links as they are, relative ones from the page's
/// folder.
fn resolve(ui: &str, current: &str, href: &str) -> Link {
    if href.starts_with('#') || href.is_empty() {
        return Link::Anchor;
    }
    let absolute = if href.contains("://") || href.starts_with("mailto:") {
        href.to_string()
    } else if href.starts_with('/') {
        format!("{ui}{href}")
    } else {
        // Relative to the page's folder: `chat` on `cli/login` is `cli/chat`.
        let folder = match current.rsplit_once('/') {
            Some((folder, _)) => format!("/docs/{folder}/"),
            None => "/docs/".to_string(),
        };
        format!("{ui}{folder}{}", href.trim_start_matches("./"))
    };
    let same_site = absolute.starts_with(ui);
    match docs::page_path(&absolute) {
        Some(path) if same_site => Link::Docs(path),
        _ => Link::Browser(absolute),
    }
}

#[cfg(test)]
mod tests {
    use super::{Link, resolve};

    const UI: &str = "https://nexus.example";

    #[test]
    fn docs_links_stay_in_the_app() {
        assert_eq!(resolve(UI, "", "/docs/cli/chat.md"), Link::Docs("cli/chat".into()));
        assert_eq!(resolve(UI, "cli/login", "chat"), Link::Docs("cli/chat".into()));
        assert_eq!(
            resolve(UI, "", "https://nexus.example/docs/api#keys"),
            Link::Docs("api".into())
        );
        assert_eq!(resolve(UI, "", "#flags"), Link::Anchor);
    }

    #[test]
    fn other_links_open_in_the_browser() {
        assert_eq!(
            resolve(UI, "", "https://github.com/nexus-xyz"),
            Link::Browser("https://github.com/nexus-xyz".into())
        );
        assert_eq!(
            resolve(UI, "", "/blog/post"),
            Link::Browser("https://nexus.example/blog/post".into())
        );
        assert_eq!(
            resolve(UI, "", "https://other.example/docs/x"),
            Link::Browser("https://other.example/docs/x".into())
        );
    }
}
