//! The Models catalog (`ui/src/components/nexus-ui/models/catalog.tsx`): every model Gateway knows, with its prices and
//! context window, and whether the organization's policies let the person use it. A search, a maker filter, and older
//! versions folded under one row, as in the model menu. A second tab lists the providers (`models_providers.rs`), as
//! the Nexus app's Models › Providers.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::{Place, View, Workspace, glyph};
use crate::catalog::{self, CatalogModel, CatalogProvider};
use crate::model::{maker_name, older_ids};
use crate::palette::Palette;

const MAX_WIDTH: f32 = 1080.;

/// The Models view's tabs: the Nexus app draws these two. Its other tabs read Nexus UI's server, in both apps.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) enum ModelsTab {
    #[default]
    Catalog,
    Providers,
}

/// What the Models view shows.
pub(super) struct ModelsState {
    pub tab: ModelsTab,
    /// The catalog last read with the model list; `None` until it arrives.
    pub models: Option<Vec<CatalogModel>>,
    /// The Gateway's providers with their rates, for the Providers tab; `None` until they arrive.
    pub providers: Option<Result<Vec<CatalogProvider>, String>>,
    pub search: Entity<InputState>,
    pub query: String,
    pub maker: Option<String>,
    pub show_older: bool,
    pub scroll: ScrollHandle,
    pub makers_scroll: ScrollHandle,
    _subscription: Subscription,
}

impl ModelsState {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> ModelsState {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search models"));
        let subscription = cx.subscribe_in(&search, window, |this, state, event: &InputEvent, _, cx| {
            if let InputEvent::Change = event {
                this.models_page.query = state.read(cx).value().to_string();
                cx.notify();
            }
        });
        ModelsState {
            tab: ModelsTab::default(),
            models: None,
            providers: None,
            search,
            query: String::new(),
            maker: None,
            show_older: false,
            scroll: ScrollHandle::new(),
            makers_scroll: ScrollHandle::new(),
            _subscription: subscription,
        }
    }
}

impl Workspace {
    pub(super) fn open_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account_gateway) = self.account_gateway() else {
            return;
        };
        if !matches!(self.view, View::Models) {
            self.visit(Place::Models);
        }
        self.reset_chat();
        self.view = View::Models;
        self.title = "Models".into();
        // The catalog comes with the model list; the Providers tab also reads the providers.
        account_gateway.list_models();
        account_gateway.list_providers();
        self.models_page.search.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    pub(super) fn render_models(&self, palette: Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let state = &self.models_page;
        let models = state.models.as_deref().unwrap_or_default();
        let mut makers: Vec<&str> = Vec::new();
        for model in models {
            if !makers.contains(&model.maker.as_str()) {
                makers.push(&model.maker);
            }
        }
        let lines: Vec<(&str, bool)> = models.iter().map(|model| (model.id.as_str(), model.allowed)).collect();
        let older = older_ids(&lines);
        let searching = !state.query.trim().is_empty();
        let shown: Vec<&CatalogModel> = models
            .iter()
            .filter(|model| state.maker.as_deref().is_none_or(|maker| model.maker == maker))
            .filter(|model| model.matches(&state.query))
            .collect();
        // A search finds older versions too; otherwise they wait behind "Show older".
        let folded = if searching || state.show_older {
            0
        } else {
            shown.iter().filter(|model| older.contains(&model.id)).count()
        };
        let rows: Vec<&CatalogModel> = shown
            .into_iter()
            .filter(|model| searching || state.show_older || !older.contains(&model.id))
            .collect();

        let chip = |id: SharedString, label: String, picked: bool| {
            div()
                .id(id)
                .flex_none()
                .h(px(28.))
                .flex()
                .items_center()
                .px(px(10.))
                .rounded(px(6.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(picked, |chip| chip.bg(palette.selected).text_color(palette.strong))
                .when(!picked, |chip| {
                    chip.text_color(palette.muted)
                        .hover(|style| style.text_color(palette.strong))
                })
                .child(label)
        };
        // One line whatever the number of makers: it scrolls sideways, fading at its right edge, rather than wrapping
        // one maker onto a second line.
        let filters = h_flex()
            .id("models-makers")
            .flex_none()
            .overflow_x_scroll()
            .track_scroll(&state.makers_scroll)
            .p(px(2.))
            .gap(px(2.))
            .child(
                chip("models-maker-all".into(), "All".into(), state.maker.is_none()).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.models_page.maker = None;
                        cx.notify();
                    },
                )),
            )
            .children(makers.iter().map(|maker| {
                let pick = maker.to_string();
                chip(
                    SharedString::from(format!("models-maker-{maker}")),
                    maker_name(maker),
                    state.maker.as_deref() == Some(*maker),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.models_page.maker = Some(pick.clone());
                    cx.notify();
                }))
            }));
        let filters = div()
            .relative()
            .flex_1()
            .min_w(px(0.))
            .h(px(32.))
            .rounded(px(8.))
            .bg(palette.hover)
            .overflow_hidden()
            .child(filters)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(24.))
                    .bg(linear_gradient(
                        90.,
                        linear_color_stop(palette.hover.opacity(0.), 0.),
                        linear_color_stop(palette.hover, 1.),
                    )),
            );
        let search = h_flex()
            .flex_none()
            .w(px(220.))
            .h(px(32.))
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
                    .child(Input::new(&state.search).appearance(false)),
            );

        let body: AnyElement = if state.models.is_none() {
            message("Reading the catalog…", "", palette).into_any_element()
        } else if models.is_empty() {
            message(
                "Couldn't read the catalog",
                "Gateway gave no models. Try again in a moment.",
                palette,
            )
            .into_any_element()
        } else if rows.is_empty() {
            message(
                &format!("Nothing matches “{}”", state.query.trim()),
                "Search looks at names, ids and makers.",
                palette,
            )
            .into_any_element()
        } else {
            let head = row_shape()
                .min_h(px(32.))
                .text_size(px(11.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.faint)
                .child(div().flex_1().min_w(px(0.)).child("Model"))
                .child(cell(260., "ID"))
                .child(number(64., "In/1M"))
                .child(number(64., "Out/1M"))
                .child(number(64., "Context"));
            v_flex()
                .gap(px(2.))
                .child(head)
                .children(rows.iter().map(|model| {
                    // Only the exceptions, as a tag by the name: a column for them stood empty on most rows.
                    let status = if !model.supported {
                        Some("Coming soon")
                    } else if !model.allowed {
                        Some("Off by policy")
                    } else {
                        None
                    };
                    let mut sub = maker_name(&model.maker);
                    if !model.default_for.is_empty() {
                        sub = format!("{sub} · default for {}", model.default_for.join(", "));
                    }
                    row_shape()
                        .id(SharedString::from(format!("models-row-{}", model.id)))
                        .min_h(px(52.))
                        .rounded(px(8.))
                        .text_size(px(13.))
                        .hover(|style| style.bg(palette.hover))
                        .when(!model.allowed, |row| row.opacity(0.6))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(0.))
                                .child(
                                    h_flex()
                                        .min_w(px(0.))
                                        .gap(px(8.))
                                        .child(div().truncate().text_color(palette.strong).child(model.name.clone()))
                                        .children(status.map(|status| {
                                            div()
                                                .flex_none()
                                                .px(px(6.))
                                                .rounded(px(4.))
                                                .bg(palette.hover)
                                                .text_size(px(11.))
                                                .line_height(px(18.))
                                                .text_color(palette.muted)
                                                .child(status)
                                        })),
                                )
                                .child(div().truncate().text_size(px(12.)).text_color(palette.faint).child(sub)),
                        )
                        .child(
                            div()
                                .w(px(260.))
                                .flex_none()
                                .truncate()
                                .font_family("Geist Mono")
                                .text_size(px(12.))
                                .text_color(palette.muted)
                                .child(model.id.clone()),
                        )
                        .child(number(64., &catalog::usd(model.input)))
                        .child(number(64., &catalog::usd(model.output)))
                        .child(number(64., &catalog::tokens(model.context)))
                }))
                .when(folded > 0 || (state.show_older && !searching), |list| {
                    let label = if state.show_older {
                        "Hide older models".to_string()
                    } else {
                        format!("Show {folded} older models")
                    };
                    list.child(
                        div()
                            .id("models-older")
                            .mt(px(10.))
                            .px(px(8.))
                            .text_size(px(13.))
                            .text_color(palette.muted)
                            .cursor_pointer()
                            .hover(|style| style.text_color(palette.strong))
                            .child(label)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.models_page.show_older = !this.models_page.show_older;
                                cx.notify();
                            })),
                    )
                })
                .into_any_element()
        };
        let tabs = h_flex()
            .flex_none()
            .p(px(2.))
            .gap(px(2.))
            .rounded(px(8.))
            .bg(palette.hover)
            .children(
                [(ModelsTab::Catalog, "Catalog"), (ModelsTab::Providers, "Providers")].map(|(tab, label)| {
                    chip(
                        SharedString::from(format!("models-tab-{label}")),
                        label.into(),
                        state.tab == tab,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.models_page.tab = tab;
                        // The Providers tab asks again after a read that failed.
                        if tab == ModelsTab::Providers && matches!(this.models_page.providers, Some(Err(_))) {
                            if let Some(account) = this.account_gateway() {
                                account.list_providers();
                            }
                        }
                        cx.notify();
                    }))
                }),
            );
        let providers = state.tab == ModelsTab::Providers;
        let body: AnyElement = if !providers {
            body
        } else {
            match &state.providers {
                Some(Err(error)) => message("The providers could not be read", error, palette).into_any_element(),
                Some(Ok(providers)) if state.models.is_some() => {
                    super::models_providers::render(providers, models, palette).into_any_element()
                }
                _ => message("Reading the catalog…", "", palette).into_any_element(),
            }
        };
        let available = models.iter().filter(|model| model.allowed).count();
        let count = if providers {
            "Who serves each model, and how they charge.".to_string()
        } else if models.is_empty() {
            "Every model on Nexus Gateway, its price and whether you can use it.".to_string()
        } else {
            format!(
                "{} models, {available} available to you. Prices are dollars per million tokens.",
                models.len()
            )
        };
        div().size_full().child(
            div()
                .id("models")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&state.scroll)
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(MAX_WIDTH))
                        // Centred in the main area, as every page there.
                        .mx_auto()
                        .px(px(48.))
                        .pt(px(28.))
                        .pb(px(40.))
                        // The search sits on the title's line, so the header is three rows: title, makers, table head.
                        .child(
                            h_flex()
                                .items_center()
                                .gap(px(24.))
                                .mb(px(18.))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .gap(px(4.))
                                        .child(
                                            div()
                                                .text_size(px(28.))
                                                .line_height(px(34.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(palette.strong)
                                                .child("Models"),
                                        )
                                        .child(div().text_size(px(13.)).text_color(palette.faint).child(count)),
                                )
                                .when(!providers, |header| header.child(search)),
                        )
                        .child(h_flex().w_full().mb(px(12.)).child(tabs))
                        .when(!providers, |page| {
                            page.child(h_flex().w_full().mb(px(12.)).child(filters))
                        })
                        .child(body),
                ),
        )
    }
}

/// A row of the table: no rules between rows, a hover fill instead.
fn row_shape() -> Div {
    h_flex().w_full().gap(px(12.)).px(px(8.))
}

fn cell(width: f32, text: &str) -> Div {
    div().w(px(width)).flex_none().truncate().child(text.to_string())
}

fn number(width: f32, text: &str) -> Div {
    cell(width, text).text_right()
}

fn message(title: &str, detail: &str, palette: Palette) -> Div {
    v_flex()
        .items_center()
        .py(px(64.))
        .gap(px(6.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(palette.strong)
                .child(title.to_string()),
        )
        .when(!detail.is_empty(), |column| {
            column.child(
                div()
                    .max_w(px(420.))
                    .text_center()
                    .text_size(px(13.))
                    .text_color(palette.faint)
                    .child(detail.to_string()),
            )
        })
}
