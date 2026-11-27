//! Artifacts, the Library (`ui/src/components/nexus-ui/library/library.tsx`): chats and pages shared with you, by you,
//! and your drafts, in four ownership tabs with a Kind menu, counts, and search over titles and people, all on one
//! toolbar row. A row opens
//! beside the list in the Workbench's Artifacts tab: a page in its frame, a shared chat as Markdown. Another
//! organization's share opens in the browser, as in the Nexus app. A switch right of the search shows the items as rows
//! or as cards (`library_gallery.rs`); the choice lasts across launches. Not here yet: the preview pane, Show older,
//! arrow keys.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Icon, Side, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Place, View, Workspace, glyph};
use crate::library::layout::Layout;
use crate::library::{self, Kind, Lists, Open, Row, Tab};
use crate::palette::Palette;

/// The list's widest, as `.library-main`.
const MAX_WIDTH: f32 = 1080.;
/// The column's padding on each side.
const SIDE: f32 = 48.;

#[path = "library_actions.rs"]
mod actions;

#[path = "library_gallery.rs"]
mod gallery;

/// What the Library shows.
pub(super) struct LibraryState {
    pub tab: Tab,
    pub kind: Kind,
    /// Rows or cards, as saved.
    pub layout: Layout,
    /// The lists last read; `None` while the first read runs.
    pub lists: Option<Result<Lists, SharedString>>,
    pub query: String,
    /// Why the last row did not open.
    pub note: Option<SharedString>,
    pub scroll: ScrollHandle,
    pub dialog: Option<Entity<actions::LibraryDialog>>,
    /// The dialog came from the open page's header (`page_menu`, `share_page`), so it shows beside the page on any
    /// screen.
    pub dialog_beside_page: bool,
}

impl LibraryState {
    pub fn new() -> LibraryState {
        LibraryState {
            tab: Tab::All,
            kind: Kind::All,
            layout: Layout::load(),
            lists: None,
            query: String::new(),
            note: None,
            scroll: ScrollHandle::new(),
            dialog: None,
            dialog_beside_page: false,
        }
    }
}

impl Workspace {
    pub(super) fn open_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.view, View::Library) {
            self.visit(Place::Library);
        }
        self.reset_chat();
        self.view = View::Library;
        self.title = "Artifacts".into();
        // Artifacts opens ready to search.
        self.focus_page(window, cx);
        // The person came to open an artifact. Prepare while they pick one, under a short intent lease.
        crate::web::prepare(super::page_canvas::STARTUP, window, cx);
        self.reload_library();
        cx.notify();
    }

    /// Read the lists again; what was read stays on screen until the answer.
    pub(super) fn reload_library(&mut self) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        let org = self.account.as_ref().map(|account| account.org_name.clone());
        account_gateway.load_library(org);
    }

    pub(super) fn library_loaded(&mut self, lists: Result<Lists, String>) {
        match (lists, &self.library.lists) {
            (Ok(lists), _) => self.library.lists = Some(Ok(lists)),
            // A failed refresh keeps the lists already read.
            (Err(_), Some(Ok(_))) => {}
            (Err(error), _) => self.library.lists = Some(Err(error.into())),
        }
        // The open page's ⋯ follows the lists: it appears for a page they now have, and goes with a deleted one.
        self.refresh_page_item();
    }

    fn open_row(&mut self, row: &Row, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if matches!(row.open, Open::SharedPage(_) | Open::Page(_)) {
            // The engine starts while Gateway answers with the page's address.
            crate::web::warm();
            // The person opened it: a design starts in full view.
            self.page_opening_auto = false;
        }
        match &row.open {
            Open::SharedPage(id) => account_gateway.frame_artifact(id.clone(), self.org_path(&row.path)),
            Open::Page(id) => account_gateway.frame_page(id.clone(), self.org_path(&row.path)),
            Open::Chat(id) => account_gateway.chat_text(id.clone(), row.title.clone(), self.org_path(&row.path)),
            Open::Browser(url) => {
                if let Some(url) = library::reader_url(&self.gateway.site_url(), url) {
                    crate::external_link::open(&url, cx);
                }
            }
        }
        self.library.note = None;
        self.library_opening = (!matches!(row.open, Open::Browser(_))).then(|| row.key.clone());
        cx.notify();
    }

    /// `/app/<org><path>`, Nexus UI's address of an organization's page.
    fn org_path(&self, path: &str) -> String {
        match &self.account {
            Some(account) => format!("/app/{}{path}", account.org),
            None => format!("/app?next={path}"),
        }
    }

    pub(super) fn render_library(&self, width: f32, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.library;
        let tab = state.tab;
        // Narrower lists drop columns, as `.library-row`'s container queries.
        let columns = if width >= 900. {
            4
        } else if width >= 560. {
            1
        } else {
            0
        };
        let tabs = h_flex()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(8.))
            .bg(palette.hover)
            .children(Tab::ALL.map(|each| {
                let count = match &state.lists {
                    Some(Ok(lists)) => lists.count(each, state.kind),
                    _ => String::new(),
                };
                h_flex()
                    .id(SharedString::from(format!("library-tab-{}", each.label())))
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(each == tab, |chip| chip.bg(palette.selected).text_color(palette.strong))
                    .when(each != tab, |chip| {
                        chip.text_color(palette.muted)
                            .hover(|style| style.text_color(palette.strong))
                    })
                    .child(each.label())
                    .when(!count.is_empty(), |chip| {
                        chip.child(div().text_size(px(11.)).text_color(palette.faint).child(count))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.library.tab = each;
                        this.library.scroll.set_offset(point(px(0.), px(0.)));
                        cx.notify();
                    }))
            }));
        // Narrow lists keep one toolbar row: the kind button drops its "Kind:" prefix and the search shrinks.
        let compact = width < COMPACT_TOOLBAR;
        let kinds = self.kind_menu(compact, palette, cx);
        let search = h_flex()
            .flex_1()
            .min_w(px(36.))
            .max_w(px(220.))
            .h(px(30.))
            .px(px(10.))
            .gap(px(8.))
            .rounded(px(8.))
            .bg(palette.hover)
            .child(glyph(Lucide::Search, palette.faint))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_size(px(13.))
                    .child(Input::new(&self.library_search).appearance(false)),
            );
        // The search and the switch end the toolbar's one row; the search gives up width before anything wraps.
        let find = h_flex()
            .flex_1()
            .min_w(px(0.))
            .justify_end()
            .gap(px(8.))
            .child(search)
            .child(self.layout_switch(palette, cx));
        // The column inside its side padding, which the gallery's cards share.
        let content = (width.min(MAX_WIDTH) - 2. * SIDE).max(0.);
        let cards = state.layout == Layout::Gallery;
        let body: AnyElement = match &state.lists {
            None if cards => gallery::skeleton(content, palette).into_any_element(),
            None => skeleton(palette).into_any_element(),
            Some(Err(error)) => message(
                "Couldn't load the Library",
                &format!("{error} Nothing shared was changed. Try again in a moment."),
                palette,
            )
            .child(
                h_flex()
                    .id("library-retry")
                    .mt(px(8.))
                    .h(px(28.))
                    .px(px(12.))
                    .rounded_full()
                    .bg(palette.selected)
                    .text_size(px(13.))
                    .cursor_pointer()
                    .hover(|style| style.bg(palette.border))
                    .active(|style| style.opacity(0.85))
                    .child("Try again")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.library.lists = None;
                        this.reload_library();
                        cx.notify();
                    })),
            )
            .into_any_element(),
            Some(Ok(lists)) => {
                let unfiltered = lists.rows(tab);
                let all = library::filter_kind(unfiltered, state.kind);
                let rows = lists.visible(tab, state.kind, &state.query);
                if all.is_empty() && state.kind != Kind::All && !unfiltered.is_empty() {
                    message(
                        &format!("No {} yet", state.kind.label().to_lowercase()),
                        state.kind.note(),
                        palette,
                    )
                    .child(
                        div()
                            .id("library-show-everything")
                            .mt(px(8.))
                            .cursor_pointer()
                            .text_size(px(13.))
                            .child("Show everything")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.library.kind = Kind::All;
                                this.library.scroll.set_offset(point(px(0.), px(0.)));
                                cx.notify();
                            })),
                    )
                    .into_any_element()
                } else if all.is_empty() {
                    let (title, detail) = match tab {
                        Tab::All => (
                            "Nothing made yet",
                            "Artifacts you and your agents make show here, with the chats shared with you. Ask for a doc, a sheet, a design or a site.",
                        ),
                        Tab::With => (
                            "Nothing shared with you yet",
                            "When someone shares a chat or an Artifact with you, your team or everyone in the organization, it shows here.",
                        ),
                        Tab::By => (
                            "You haven't shared anything yet",
                            "Open a chat and choose Share. What you share shows here, with who can read it.",
                        ),
                        Tab::Pages => (
                            "No drafts yet",
                            "Ask a session to write a doc or build a site, or publish one from the CLI with: nexus pages publish report.html",
                        ),
                    };
                    message(title, detail, palette).into_any_element()
                } else if rows.is_empty() {
                    message(
                        &format!("Nothing matches “{}”", state.query.trim()),
                        "Try another word, or clear the filter.",
                        palette,
                    )
                    .into_any_element()
                } else if cards {
                    self.render_cards(rows, content, palette, cx)
                } else {
                    let now = library::chrono_lite::now();
                    let head = row_shape()
                        .min_h(px(32.))
                        .text_size(px(11.))
                        .text_color(palette.faint)
                        .child(div().w(px(24.)).flex_none())
                        .child(div().flex_1().min_w(px(0.)).child("Title"))
                        .when(columns >= 1, |head| {
                            head.child(cell(140., if tab == Tab::By { "Audience" } else { "Shared by" }))
                        })
                        .when(columns >= 4, |head| {
                            head.child(cell(140., if tab == Tab::By { "Type" } else { "Audience" }))
                        })
                        .child(div().w(px(64.)).flex_none().text_right().child("Updated"))
                        .child(div().w(px(44.)).flex_none().text_right().child("More"));
                    v_flex()
                        .child(head)
                        .children(rows.into_iter().enumerate().map(|(ix, row)| {
                            let opening = self.library_opening.as_deref() == Some(row.key.as_str());
                            let open = row.clone();
                            let context_row = row.clone();
                            let workspace = cx.entity().downgrade();
                            let page = matches!(row.open, Open::SharedPage(_) | Open::Page(_));
                            row_shape()
                                .id(("library-row", ix))
                                .when(page, |line| {
                                    line.on_hover(|hovered, window, cx| {
                                        crate::web::prepare_on_hover_then(
                                            super::page_canvas::STARTUP,
                                            *hovered,
                                            window,
                                            cx,
                                            || {},
                                        )
                                    })
                                })
                                .min_h(px(52.))
                                .text_size(px(13.))
                                .tab_index(0)
                                .cursor_pointer()
                                .when(opening, |line| line.bg(palette.hover))
                                .hover(|style| style.bg(palette.hover))
                                .focus_visible(|style| style.bg(palette.hover))
                                .active(|style| style.bg(palette.selected))
                                .when(row.revoked, |line| line.opacity(0.6))
                                .child(
                                    div()
                                        .w(px(24.))
                                        .flex_none()
                                        .child(glyph(kind_icon(&row), palette.muted)),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .child(
                                            h_flex()
                                                .gap(px(6.))
                                                .min_w(px(0.))
                                                .child(
                                                    div()
                                                        .truncate()
                                                        .text_color(palette.strong)
                                                        .child(row.title.clone()),
                                                )
                                                .when(row.outside, |name| name.child(pill("Outside", palette)))
                                                .when(row.revoked, |name| name.child(pill("Revoked", palette))),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(12.))
                                                .text_color(palette.faint)
                                                .child(kind_line(&row)),
                                        ),
                                )
                                .when(columns >= 1, |line| {
                                    line.child(cell(140., if tab == Tab::By { &row.audience } else { &row.shared_by }))
                                })
                                .when(columns >= 4, |line| {
                                    line.child(cell(140., if tab == Tab::By { kind_name(&row) } else { &row.audience }))
                                })
                                .child(
                                    div()
                                        .w(px(64.))
                                        .flex_none()
                                        .text_right()
                                        .text_color(palette.faint)
                                        .child(library::short_ago(&row.updated_at, now)),
                                )
                                .child(self.library_more(row, cx))
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
                        }))
                        .into_any_element()
                }
            }
        };
        div().size_full().child(
            div()
                .id("library")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&state.scroll)
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(MAX_WIDTH))
                        // Centred in the main area, as every page there.
                        .mx_auto()
                        .px(px(SIDE))
                        .pt(px(32.))
                        .pb(px(48.))
                        .child(
                            v_flex()
                                .gap(px(4.))
                                .mb(px(24.))
                                .child(
                                    div()
                                        .text_size(px(28.))
                                        .line_height(px(34.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(palette.strong)
                                        .child("Artifacts"),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(palette.faint)
                                        .child("Chats, docs and sites shared with you and by you."),
                                ),
                        )
                        .child(
                            h_flex()
                                .gap(px(8.))
                                .mb(px(16.))
                                .child(tabs.flex_none())
                                .child(kinds)
                                .child(find),
                        )
                        .when_some(state.note.clone(), |column, note| {
                            column.child(
                                div()
                                    .mb(px(8.))
                                    .text_size(px(13.))
                                    .text_color(palette.destructive)
                                    .child(note),
                            )
                        })
                        .child(body),
                ),
        )
    }
}

impl Workspace {
    /// The kind filter: one button that opens a menu of kinds, each with its icon and its count in the current tab.
    fn kind_menu(&self, compact: bool, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let current = self.library.kind;
        let workspace = cx.entity().downgrade();
        let button = Button::new("library-kind")
            .ghost()
            .compact()
            .dropdown_caret(true)
            .h(px(30.))
            .px(px(10.))
            .rounded(px(8.))
            .bg(palette.hover)
            .text_size(px(13.))
            .text_color(if current == Kind::All {
                palette.muted
            } else {
                palette.strong
            })
            .label(kind_button_label(current, compact))
            .accessibility_label("Filter by kind")
            .dropdown_menu(move |mut menu, _, cx| {
                let Some(workspace) = workspace.upgrade() else {
                    return menu;
                };
                let this = workspace.read(cx);
                // Each kind's count in the tab on screen, as the tabs count the kind on screen.
                let counts = match &this.library.lists {
                    Some(Ok(lists)) => Kind::ALL.map(|kind| lists.count(this.library.tab, kind)),
                    _ => Kind::ALL.map(|_| String::new()),
                };
                menu = menu.check_side(Side::Right).min_w(px(220.));
                for (kind, count) in Kind::ALL.into_iter().zip(counts) {
                    if kind == Kind::Site {
                        menu = menu.separator();
                    }
                    let label = kind_menu_label(kind);
                    let workspace = workspace.downgrade();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| {
                            h_flex().w_full().gap(px(12.)).child(label).child(
                                div()
                                    .ml_auto()
                                    .text_size(px(11.))
                                    .text_color(palette.faint)
                                    .child(count.clone()),
                            )
                        })
                        .icon(Icon::new(kind_glyph(kind)))
                        .checked(kind == current)
                        .on_click(move |_, _, cx| {
                            // The window may have closed while the menu was open; nothing is left to filter then.
                            if let Some(workspace) = workspace.upgrade() {
                                workspace.update(cx, |this, cx| {
                                    this.library.kind = kind;
                                    this.library.scroll.set_offset(point(px(0.), px(0.)));
                                    cx.notify();
                                });
                            }
                        }),
                    );
                }
                menu
            });
        h_flex()
            .flex_none()
            .gap(px(2.))
            .child(button)
            .when(current != Kind::All, |group| {
                group.child(
                    Button::new("library-kind-clear")
                        .ghost()
                        .icon(Lucide::X)
                        .w(px(26.))
                        .h(px(26.))
                        .accessibility_label("Show all kinds")
                        .tooltip("Show all kinds")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.library.kind = Kind::All;
                            this.library.scroll.set_offset(point(px(0.), px(0.)));
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
}

/// Below this width the toolbar's kind button drops its "Kind:" prefix.
const COMPACT_TOOLBAR: f32 = 900.;

/// The kind button's text: "Kind: All", "Kind: Docs"; narrow, "Kind" or "Docs".
fn kind_button_label(kind: Kind, compact: bool) -> String {
    match (kind, compact) {
        (Kind::All, true) => "Kind".into(),
        (kind, true) => kind.label().into(),
        (kind, false) => format!("Kind: {}", kind.label()),
    }
}

/// A row of the kind menu: "All kinds", then each kind's plural.
fn kind_menu_label(kind: Kind) -> &'static str {
    match kind {
        Kind::All => "All kinds",
        kind => kind.label(),
    }
}

/// A kind's icon, as Nexus UI's `ROW_ICON`; All is the grid of every kind.
fn kind_glyph(kind: Kind) -> Lucide {
    match kind {
        Kind::All => Lucide::LayoutGrid,
        Kind::Chat => Lucide::MessageSquare,
        Kind::Doc => Lucide::FileText,
        Kind::Site => Lucide::AppWindow,
        Kind::Sheet => Lucide::Table,
        Kind::Board => Lucide::LayoutDashboard,
    }
}

/// A row of the list and its columns' gaps; no line under it, the rows' spacing sets them apart.
fn row_shape() -> Div {
    h_flex().w_full().gap(px(16.)).px(px(12.)).rounded(px(8.))
}

fn kind(row: &Row) -> Kind {
    row.artifact_kind()
}

/// A row's icon, as Nexus UI's `ROW_ICON`.
fn kind_icon(row: &Row) -> Lucide {
    kind_glyph(kind(row))
}

/// The Type column: "Chat", "Doc", "Site".
fn kind_name(row: &Row) -> &'static str {
    match kind(row) {
        Kind::Chat => row.kind,
        kind => kind.name(),
    }
}

/// The line under the title: a chat's version and model; "Doc", or "Site · HTML" (`pageSub`).
fn kind_line(row: &Row) -> String {
    row.sub.clone()
}

fn cell(width: f32, text: &str) -> Div {
    div().w(px(width)).flex_none().truncate().child(text.to_string())
}

fn pill(text: &'static str, palette: Palette) -> Div {
    div()
        .flex_none()
        .px(px(6.))
        .rounded(px(6.))
        .bg(palette.hover)
        .text_size(px(11.))
        .text_color(palette.muted)
        .child(text)
}

fn message(title: &str, detail: &str, palette: Palette) -> Div {
    v_flex()
        .items_center()
        .py(px(64.))
        .gap(px(8.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child(title.to_string()),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_center()
                .text_size(px(13.))
                .text_color(palette.faint)
                .child(detail.to_string()),
        )
}

/// Six grey rows the size of the list while the first read runs.
fn skeleton(palette: Palette) -> Div {
    v_flex().children((0..6).map(|ix| {
        h_flex()
            .min_h(px(52.))
            .gap(px(12.))
            .px(px(12.))
            .rounded(px(8.))
            .child(div().size(px(16.)).rounded(px(4.)).bg(palette.hover))
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .h(px(10.))
                            .w(px(180. + (ix % 3) as f32 * 60.))
                            .rounded(px(4.))
                            .bg(palette.hover),
                    )
                    .child(div().h(px(8.)).w(px(110.)).rounded(px(4.)).bg(palette.hover)),
            )
    }))
}

#[cfg(test)]
mod tests {
    use super::{Kind, kind, kind_button_label, kind_line, kind_menu_label, kind_name};

    #[test]
    fn the_kind_button_names_the_filter_and_drops_its_prefix_when_narrow() {
        assert_eq!(kind_button_label(Kind::All, false), "Kind: All");
        assert_eq!(kind_button_label(Kind::Doc, false), "Kind: Docs");
        assert_eq!(kind_button_label(Kind::All, true), "Kind");
        assert_eq!(kind_button_label(Kind::Sheet, true), "Sheets");
        assert_eq!(kind_menu_label(Kind::All), "All kinds");
        assert_eq!(kind_menu_label(Kind::Board), "Designs");
    }
    use crate::library::{Open, Row};

    fn row(page: bool, format: Option<&str>) -> Row {
        Row {
            key: "k".into(),
            page,
            mine: false,
            owned_page: None,
            source_page: None,
            link: None,
            title: "T".into(),
            sub: if page {
                match format {
                    Some("md") => "Doc".into(),
                    Some("sheet") => "Sheet".into(),
                    Some("board") => "Design".into(),
                    Some("jsx") => "Site · React".into(),
                    _ => "Site · HTML".into(),
                }
            } else {
                String::new()
            },
            format: format.map(str::to_string),
            shared_by: "You".into(),
            author_email: String::new(),
            shares: Vec::new(),
            audience: "–".into(),
            kind: if page { "Page" } else { "Chat" },
            version: "v1".into(),
            updated_at: String::new(),
            outside: false,
            revoked: false,
            path: "/pages/p".into(),
            open: Open::Page("p".into()),
        }
    }

    #[test]
    fn tells_a_doc_from_a_site() {
        let doc = row(true, Some("md"));
        let site = row(true, Some("jsx"));
        let mut chat = row(false, None);
        chat.sub = "v3 · claude-sonnet-5.5".into();
        assert_eq!(kind(&doc), Kind::Doc);
        assert_eq!(kind(&site), Kind::Site);
        assert_eq!(kind(&chat), Kind::Chat);
        assert_eq!(kind_line(&doc), "Doc");
        assert_eq!(kind_line(&site), "Site · React");
        assert_eq!(kind_line(&chat), "v3 · claude-sonnet-5.5");
        assert_eq!(kind_name(&site), "Site");
        assert_eq!(kind_name(&chat), "Chat");
    }
}
