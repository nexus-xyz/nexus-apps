//! Models › Providers (`ui/src/components/nexus-ui/explorer/providers-tab.tsx`): the providers Nexus
//! Gateway calls, how each charges beyond a model's input and output price, and the models each serves today.
//!
//! The providers, their order, status, rates, API host and what Gateway calls them for are live from
//! `GET /v1/providers`, and the models and their prices from the catalog this view already reads
//! (`GET /v1/models?status=all`). `PROFILES` adds only the text the Gateway does not serve: who each provider is, as
//! the web tab's `explorer/providers.ts`. Change the two together.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

use super::model_menu::maker_mark;
use crate::catalog::{self, CacheWrite, CatalogModel, CatalogProvider, ProviderRates};
use crate::palette::Palette;

/// Whether Gateway calls a provider today.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Status {
    Active,
    Inactive,
    /// Only typed decisions (`POST /v1/decisions`).
    Decisions,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Active => "Active",
            Status::Inactive => "Not yet available",
            Status::Decisions => "Typed decisions only",
        }
    }
}

/// Who a provider is: the text the Gateway does not serve, as `PROFILES` in `explorer/providers.ts`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Profile {
    pub about: &'static str,
    pub founded: Option<&'static str>,
    pub hq: Option<&'static str>,
    pub site: Option<&'static str>,
}

/// One provider as the tab shows it: the Gateway's, with its profile when the tab has one.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Provider {
    /// Its id in model ids, stored keys and spend records.
    pub id: String,
    pub name: String,
    pub status: Status,
    pub profile: Option<&'static Profile>,
    /// `None` from a Gateway that does not publish rates.
    pub rates: Option<ProviderRates>,
    /// The host of the API Gateway calls it on; `None` from an older Gateway.
    pub api_host: Option<String>,
}

const PROFILES: &[(&str, Profile)] = &[
    (
        "anthropic",
        Profile {
            about: "An AI safety and research company that builds the Claude family of models, known for coding, long \
                    context and careful behaviour.",
            founded: Some("2021"),
            hq: Some("San Francisco"),
            site: Some("https://www.anthropic.com"),
        },
    ),
    (
        "openai",
        Profile {
            about: "The lab behind GPT and ChatGPT, which brought large language models to hundreds of millions of \
                    people.",
            founded: Some("2015"),
            hq: Some("San Francisco"),
            site: Some("https://openai.com"),
        },
    ),
    (
        "baseten",
        Profile {
            about: "Serves open-weight models through its Model APIs. Nexus calls it for DeepSeek's models.",
            founded: None,
            hq: Some("San Francisco"),
            site: Some("https://www.baseten.co"),
        },
    ),
    (
        "deepinfra",
        Profile {
            about: "Serves open-weight models through an OpenAI-compatible API. Nexus will call it for DeepSeek's \
                    models.",
            founded: None,
            hq: Some("Palo Alto"),
            site: Some("https://deepinfra.com"),
        },
    ),
    (
        "google",
        Profile {
            about: "Serves its Gemini models through the Gemini API. Nexus will call it for Gemini 3.8 Flash.",
            founded: None,
            hq: Some("Mountain View"),
            site: Some("https://ai.google.dev"),
        },
    ),
    (
        "typesafe",
        Profile {
            about: "Serves typed decisions (POST /v1/decisions) through its model Jev, on Nexus's own key for every \
                    organization, each charged at list price.",
            founded: None,
            hq: None,
            site: None,
        },
    ),
];

/// The Gateway's providers in its order, each with its profile, and those that serve only typed decisions last.
pub(super) fn providers(listed: &[CatalogProvider]) -> Vec<Provider> {
    let shown: Vec<Provider> = listed
        .iter()
        .map(|provider| {
            let profile = PROFILES
                .iter()
                .find(|(id, _)| *id == provider.id)
                .map(|(_, profile)| profile);
            let status = match (provider.decisions, provider.active) {
                (true, _) => Status::Decisions,
                (false, true) => Status::Active,
                (false, false) => Status::Inactive,
            };
            Provider {
                id: provider.id.clone(),
                name: provider.name.clone(),
                status,
                profile,
                rates: provider.rates,
                api_host: provider.api_host.clone(),
            }
        })
        .collect();
    let (decisions, models): (Vec<Provider>, Vec<Provider>) = shown
        .into_iter()
        .partition(|provider| provider.status == Status::Decisions);
    models.into_iter().chain(decisions).collect()
}

/// The web's green "Active" pill (`--nx-success`), readable on both themes.
const ACTIVE: Hsla = Hsla {
    h: 142. / 360.,
    s: 0.6,
    l: 0.42,
    a: 1.,
};

pub(super) const INTRO: &str = "The providers Nexus Gateway calls on your behalf: who they are, how they charge beyond \
                                each model's input and output price, and the models each serves today.";
const FOOTER: &str =
    "Rates: Nexus Gateway's catalog, the provider's list prices. Models and prices: Nexus Gateway, live.";

/// The models a provider serves today, in Gateway's order: supported ones whose provider is it.
pub(super) fn served<'a>(models: &'a [CatalogModel], provider: &str) -> Vec<&'a CatalogModel> {
    models
        .iter()
        .filter(|model| model.supported && model.provider == provider)
        .collect()
}

/// "Founded 2021, based in San Francisco." as the web writes it, with either part missing.
pub(super) fn origin(profile: &Profile) -> String {
    match (profile.founded, profile.hq) {
        (Some(founded), Some(hq)) => format!(" Founded {founded}, based in {hq}."),
        (Some(founded), None) => format!(" Founded {founded}."),
        (None, Some(hq)) => format!(" Based in {hq}."),
        (None, None) => String::new(),
    }
}

/// A site without its scheme and `www.`: `anthropic.com`.
pub(super) fn site_label(site: &str) -> String {
    let bare = site
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    bare.trim_end_matches('/').to_string()
}

/// The four figures under a provider: (label, value, note). Without rates, each rate is a dash.
pub(super) fn stats(provider: &Provider, models: usize) -> [(&'static str, String, String); 4] {
    let unpublished = || ("—".to_string(), "Not published".to_string());
    let rates = provider.rates;
    let cache_read = match rates.map(|rates| rates.cache_read_share) {
        Some(Some(share)) => (
            format!("{}%", trim(share * 100.)),
            "Of the model's input rate".to_string(),
        ),
        Some(None) => ("—".to_string(), "Priced per model".to_string()),
        None => unpublished(),
    };
    let cache_write = match rates.and_then(|rates| rates.cache_write) {
        Some(CacheWrite::Multipliers { five_minutes, one_hour }) => (
            format!("{}× / {}×", trim(five_minutes), trim(one_hour)),
            "Of the input rate, for 5 minutes / 1 hour".to_string(),
        ),
        Some(CacheWrite::ModelRate) => (
            "Model rate".to_string(),
            "Each model's own cache-write rate".to_string(),
        ),
        None => unpublished(),
    };
    let web_search = match rates.map(|rates| rates.web_search_usd) {
        Some(Some(usd)) => (format!("${}", trim(usd * 1000.)), "Per 1,000 searches".to_string()),
        Some(None) => ("—".to_string(), "Not offered".to_string()),
        None => unpublished(),
    };
    let host = provider.api_host.clone().unwrap_or_default();
    [
        ("Cache read", cache_read.0, cache_read.1),
        ("Cache write", cache_write.0, cache_write.1),
        ("Web search", web_search.0, web_search.1),
        ("Models on Nexus", models.to_string(), host),
    ]
}

/// A number with no trailing zeros: `10`, `1.25`.
fn trim(value: f64) -> String {
    let text = format!("{:.2}", value);
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The tab's body: one section per provider, then the sources.
pub(super) fn render(listed: &[CatalogProvider], models: &[CatalogModel], palette: Palette) -> Div {
    v_flex()
        .gap(px(36.))
        .child(
            div()
                .max_w(px(640.))
                .text_size(px(13.))
                .text_color(palette.muted)
                .child(INTRO),
        )
        .children(
            providers(listed)
                .iter()
                .map(|provider| section(provider, &served(models, &provider.id), palette)),
        )
        .child(div().text_size(px(12.)).text_color(palette.faint).child(FOOTER))
}

fn section(provider: &Provider, models: &[&CatalogModel], palette: Palette) -> Div {
    let pill = div()
        .flex_none()
        .px(px(8.))
        .rounded(px(10.))
        .text_size(px(11.))
        .line_height(px(20.))
        .when(provider.status == Status::Active, |pill| {
            pill.bg(ACTIVE.opacity(0.14)).text_color(ACTIVE)
        })
        .when(provider.status != Status::Active, |pill| {
            pill.bg(palette.hover).text_color(palette.muted)
        })
        .child(provider.status.label());
    let head = h_flex()
        .gap(px(8.))
        .child(maker_mark(&provider.id, 16., palette))
        .child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.strong)
                .child(provider.name.clone()),
        )
        .child(pill);
    let about = provider.profile.map(|profile| {
        div()
            .max_w(px(720.))
            .text_size(px(13.))
            .text_color(palette.foreground)
            .child(format!("{}{}", profile.about, origin(profile)))
    });
    let site = provider.profile.and_then(|profile| profile.site).map(|site| {
        let url = site.to_string();
        div()
            .id(SharedString::from(format!("provider-site-{}", provider.id)))
            .text_size(px(13.))
            .text_color(palette.muted)
            .cursor_pointer()
            .hover(|style| style.underline())
            .child(site_label(site))
            .on_click(move |_, _, cx| crate::external_link::open(&url, cx))
    });
    let figures = h_flex()
        .gap(px(12.))
        .children(
            stats(provider, models.len())
                .into_iter()
                .enumerate()
                .map(|(index, (label, value, note))| {
                    v_flex()
                        .flex_1()
                        .min_w(px(0.))
                        .gap(px(2.))
                        .p(px(12.))
                        .rounded(px(10.))
                        .bg(palette.hover)
                        .child(div().text_size(px(11.)).text_color(palette.faint).child(label))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette.strong)
                                .child(value),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(palette.faint)
                                // The host is one word in monospace; the other notes wrap.
                                .when(index == 3, |note| note.truncate().font_family("Geist Mono"))
                                .child(note),
                        )
                }),
        );
    let table: AnyElement = if models.is_empty() {
        div()
            .text_size(px(13.))
            .text_color(palette.faint)
            .child(if provider.status == Status::Inactive {
                "No models yet: Nexus Gateway does not call this provider until its models are in the catalog."
            } else {
                "No models listed for this provider."
            })
            .into_any_element()
    } else {
        let head = row()
            .min_h(px(32.))
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(palette.faint)
            .child(div().flex_1().min_w(px(0.)).child("Model"))
            .child(number("Input"))
            .child(number("Output"))
            .child(number("Context"))
            .child(number("Max output"));
        v_flex()
            .gap(px(2.))
            .child(head)
            .children(models.iter().map(|model| {
                row()
                    .min_h(px(48.))
                    .rounded(px(8.))
                    .text_size(px(13.))
                    .hover(|style| style.bg(palette.hover))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(px(0.))
                            .gap(px(8.))
                            .child(maker_mark(&model.maker, 14., palette))
                            .child(
                                v_flex()
                                    .min_w(px(0.))
                                    .child(div().truncate().text_color(palette.strong).child(model.name.clone()))
                                    .child(
                                        div()
                                            .truncate()
                                            .font_family("Geist Mono")
                                            .text_size(px(11.))
                                            .text_color(palette.faint)
                                            .child(model.id.clone()),
                                    ),
                            ),
                    )
                    .child(number(&catalog::usd(model.input)))
                    .child(number(&catalog::usd(model.output)))
                    .child(number(&catalog::tokens(model.context)))
                    .child(number(&catalog::tokens(model.max_output)))
            }))
            .child(
                div()
                    .mt(px(6.))
                    .px(px(8.))
                    .text_size(px(12.))
                    .text_color(palette.faint)
                    .child("List prices, USD per million tokens."),
            )
            .into_any_element()
    };
    v_flex()
        .gap(px(12.))
        .child(head)
        .child(v_flex().gap(px(2.)).children(about).children(site))
        .child(figures)
        .child(table)
}

fn row() -> Div {
    h_flex().w_full().gap(px(12.)).px(px(8.))
}

fn number(text: &str) -> Div {
    div().w(px(80.)).flex_none().text_right().child(text.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Status, origin, providers, served, site_label, stats};
    use crate::catalog::{parse, parse_providers};
    use serde_json::json;

    /// `GET /v1/providers` without a key, in the Gateway's order (Gateway's own tests check these values).
    fn listed() -> serde_json::Value {
        let per_model = json!({ "cacheReadShare": null, "cacheWrite": { "rule": "model_rate" }, "webSearchUsd": null });
        json!({ "object": "list", "data": [
            { "id": "anthropic", "name": "Anthropic", "status": "active", "rates": {
                "cacheReadShare": 0.1, "cacheWrite": { "rule": "multipliers", "fiveMinutes": 1.25, "oneHour": 2.0 },
                "webSearchUsd": "0.010000" }, "apiHost": "api.anthropic.com", "serving": "model_api" },
            { "id": "openai", "name": "OpenAI", "status": "active", "rates": {
                "cacheReadShare": 0.1, "cacheWrite": { "rule": "model_rate" }, "webSearchUsd": "0.010000" },
              "apiHost": "api.openai.com", "serving": "model_api" },
            { "id": "typesafe", "name": "TypeSafe", "status": "active", "rates": {
                "cacheReadShare": 0.1, "cacheWrite": { "rule": "model_rate" }, "webSearchUsd": null },
              "apiHost": "api.typesafe.ai", "serving": "decisions" },
            { "id": "baseten", "name": "Baseten", "status": "active", "rates": per_model,
              "apiHost": "inference.baseten.co", "serving": "model_api" },
            { "id": "deepinfra", "name": "DeepInfra", "status": "inactive", "rates": per_model,
              "apiHost": "api.deepinfra.com", "serving": "model_api" },
            { "id": "google", "name": "Google", "status": "inactive", "rates": per_model,
              "apiHost": "generativelanguage.googleapis.com", "serving": "model_api" }
        ]})
    }

    #[test]
    fn groups_live_models_by_provider() {
        let catalog = parse(&json!({ "data": [
            { "id": "anthropic/claude-opus-5-5", "owned_by": "anthropic", "status": "supported",
              "context_window": 200000, "max_output_tokens": 64000 },
            { "id": "deepseek/deepseek-v4", "owned_by": "deepseek", "provider": "baseten", "status": "supported" },
            { "id": "anthropic/claude-next", "owned_by": "anthropic", "status": "coming_soon" }
        ]}));
        let anthropic = served(&catalog, "anthropic");
        assert_eq!(anthropic.len(), 1, "coming-soon models are left out");
        assert_eq!(anthropic[0].max_output, Some(64000));
        assert_eq!(
            served(&catalog, "baseten")[0].id,
            "deepseek/deepseek-v4",
            "provider wins over owned_by"
        );
        assert!(served(&catalog, "google").is_empty());
    }

    /// The figures and order each provider showed while this tab kept its own copy of the catalog's rates.
    #[test]
    fn writes_the_figures_as_the_web_tab() {
        let shown = providers(&parse_providers(&listed()));
        assert_eq!(
            shown.iter().map(|p| (p.id.as_str(), p.status)).collect::<Vec<_>>(),
            [
                ("anthropic", Status::Active),
                ("openai", Status::Active),
                ("baseten", Status::Active),
                ("deepinfra", Status::Inactive),
                ("google", Status::Inactive),
                ("typesafe", Status::Decisions),
            ]
        );
        // Each rate's value, then the host under the model count.
        let figures: Vec<[String; 4]> = shown
            .iter()
            .map(|provider| {
                let [read, write, search, host] = stats(provider, 0);
                [read.1, write.1, search.1, host.2]
            })
            .collect();
        assert_eq!(
            figures,
            [
                ["10%", "1.25× / 2×", "$10", "api.anthropic.com"],
                ["10%", "Model rate", "$10", "api.openai.com"],
                ["—", "Model rate", "—", "inference.baseten.co"],
                ["—", "Model rate", "—", "api.deepinfra.com"],
                ["—", "Model rate", "—", "generativelanguage.googleapis.com"],
                ["10%", "Model rate", "—", "api.typesafe.ai"],
            ]
            .map(|row| row.map(String::from))
        );
        let anthropic = stats(&shown[0], 3);
        assert_eq!(anthropic[3].1, "3");
        assert_eq!(
            anthropic.map(|(_, _, note)| note),
            [
                "Of the model's input rate",
                "Of the input rate, for 5 minutes / 1 hour",
                "Per 1,000 searches",
                "api.anthropic.com"
            ]
            .map(String::from)
        );
        let baseten = stats(&shown[2], 0);
        assert_eq!(
            (baseten[0].2.as_str(), baseten[2].2.as_str()),
            ("Priced per model", "Not offered")
        );
        assert_eq!(
            origin(shown[0].profile.unwrap()),
            " Founded 2021, based in San Francisco."
        );
        assert_eq!(origin(shown[2].profile.unwrap()), " Based in San Francisco.");
        assert_eq!(site_label("https://www.anthropic.com"), "anthropic.com");
    }

    #[test]
    fn shows_a_provider_without_a_profile_and_a_gateway_without_rates() {
        // No profile: its published host and serving still show.
        let shown = providers(&parse_providers(&json!({ "data": [
            { "id": "acme", "name": "Acme", "status": "active", "apiHost": "api.acme.dev", "serving": "decisions",
              "rates": { "cacheReadShare": null, "cacheWrite": { "rule": "model_rate" }, "webSearchUsd": null } }
        ]})));
        assert_eq!((shown[0].status, shown[0].profile), (Status::Decisions, None));
        assert_eq!(stats(&shown[0], 1)[3].2, "api.acme.dev");
        // An older Gateway: no rates, host or serving.
        let shown = providers(&parse_providers(&json!({ "data": [
            { "id": "acme", "name": "Acme", "status": "active" }
        ]})));
        assert_eq!(
            (shown[0].name.as_str(), shown[0].status, shown[0].profile),
            ("Acme", Status::Active, None)
        );
        assert_eq!(
            stats(&shown[0], 2).map(|(_, value, note)| format!("{value} {note}")),
            ["— Not published", "— Not published", "— Not published", "2 "].map(String::from)
        );
        // A cache-write rule this build does not know: only that figure is a dash.
        let shown = providers(&parse_providers(&json!({ "data": [
            { "id": "acme", "name": "Acme", "status": "active", "rates": {
                "cacheReadShare": 0.1, "cacheWrite": { "rule": "tiered" }, "webSearchUsd": "0.010000" } }
        ]})));
        assert_eq!(
            stats(&shown[0], 2).map(|(_, value, _)| value)[..3],
            ["10%", "—", "$10"].map(String::from)
        );
    }
}
