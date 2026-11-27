//! The Models catalog (Nexus UI's `catalog.tsx`): every model of `GET /v1/models?status=all`,
//! not only the chat ones the model menu lists, with its prices and context window.

use serde_json::Value;

use crate::model::{maker_name, provider_of, text};

/// One model of the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    /// Its maker (`owned_by`).
    pub maker: String,
    /// Who serves it (`provider`, else its maker): the Providers tab groups by it.
    pub provider: String,
    /// Supported; the others are coming soon.
    pub supported: bool,
    /// Whether the person may use it under the organization's policies.
    pub allowed: bool,
    /// Dollars per million input and output tokens.
    pub input: Option<f64>,
    pub output: Option<f64>,
    /// Input and output tokens together, at most.
    pub context: Option<u64>,
    /// Output tokens in one answer, at most.
    pub max_output: Option<u64>,
    /// Whose default it is: chat, Claude Code, Codex.
    pub default_for: Vec<String>,
}

impl CatalogModel {
    pub fn from_json(model: &Value) -> Option<CatalogModel> {
        let id = text(model, "id").filter(|id| !id.is_empty())?;
        let name = text(model, "name")
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.clone());
        let maker = text(model, "owned_by")
            .filter(|maker| !maker.is_empty())
            .unwrap_or_else(|| provider_of(&id));
        let provider = text(model, "provider")
            .filter(|provider| !provider.is_empty())
            .unwrap_or_else(|| maker.clone());
        let supported = model.get("status").and_then(Value::as_str) == Some("supported");
        let pricing = model.get("pricing");
        let price = |name: &str| pricing.and_then(|pricing| pricing.get(name)).and_then(Value::as_f64);
        Some(CatalogModel {
            id,
            name,
            maker,
            provider,
            supported,
            allowed: supported && model.get("allowed").and_then(Value::as_bool).unwrap_or(true),
            input: price("input"),
            output: price("output"),
            context: model.get("context_window").and_then(Value::as_u64),
            max_output: model.get("max_output_tokens").and_then(Value::as_u64),
            default_for: model
                .get("default_for")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        // An agent's id reads as its name; `chat` and any other kind as they come.
                        .map(|kind| {
                            nexus_client::agent_entries::Agent::parse(kind)
                                .map_or(kind, |agent| agent.entry().identity.name)
                                .to_string()
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Whether it matches a search: its name, id or maker.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || self.name.to_lowercase().contains(&query)
            || self.id.to_lowercase().contains(&query)
            || maker_name(&self.maker).to_lowercase().contains(&query)
    }
}

/// The catalog's models, newest-looking first within each maker as the Gateway lists them.
pub fn parse(page: &Value) -> Vec<CatalogModel> {
    page.get("data")
        .and_then(Value::as_array)
        .map(|models| models.iter().filter_map(CatalogModel::from_json).collect())
        .unwrap_or_default()
}

/// One provider of `GET /v1/providers`, with its rates.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogProvider {
    /// Its id in model ids, stored keys and spend records.
    pub id: String,
    pub name: String,
    /// Whether Gateway calls it.
    pub active: bool,
    /// `None` from a Gateway that does not publish rates.
    pub rates: Option<ProviderRates>,
    /// The host of the API Gateway calls it on (`api.anthropic.com`); `None` from an older Gateway.
    pub api_host: Option<String>,
    /// Gateway calls it only for typed decisions (`serving: "decisions"`).
    pub decisions: bool,
}

/// What a provider charges beyond a model's input and output price, where the model states none of its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProviderRates {
    /// A cache read, as a share of the model's input rate; `None` where each model has its own.
    pub cache_read_share: Option<f64>,
    /// `None` for a rule this build does not know.
    pub cache_write: Option<CacheWrite>,
    /// One hosted web search, in dollars; `None` where the provider hosts none.
    pub web_search_usd: Option<f64>,
}

/// A cache write: multipliers on the input rate by how long the entry lives, or each model's own rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CacheWrite {
    Multipliers { five_minutes: f64, one_hour: f64 },
    ModelRate,
}

impl CatalogProvider {
    pub fn from_json(provider: &Value) -> Option<CatalogProvider> {
        let id = text(provider, "id").filter(|id| !id.is_empty())?;
        let name = text(provider, "name")
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.clone());
        Some(CatalogProvider {
            id,
            name,
            active: provider.get("status").and_then(Value::as_str) == Some("active"),
            rates: provider.get("rates").and_then(ProviderRates::from_json),
            api_host: text(provider, "apiHost").filter(|host| !host.is_empty()),
            decisions: provider.get("serving").and_then(Value::as_str) == Some("decisions"),
        })
    }
}

impl ProviderRates {
    fn from_json(rates: &Value) -> Option<ProviderRates> {
        if !rates.is_object() {
            return None;
        }
        let write = rates.get("cacheWrite");
        let multiple = |name: &str| write.and_then(|write| write.get(name)).and_then(Value::as_f64);
        // A rule a newer Gateway adds leaves the cache write unknown, and the other rates stand.
        let cache_write = match write.and_then(|write| write.get("rule")).and_then(Value::as_str) {
            Some("multipliers") => multiple("fiveMinutes")
                .zip(multiple("oneHour"))
                .map(|(five_minutes, one_hour)| CacheWrite::Multipliers { five_minutes, one_hour }),
            Some("model_rate") => Some(CacheWrite::ModelRate),
            _ => None,
        };
        Some(ProviderRates {
            cache_read_share: rates.get("cacheReadShare").and_then(Value::as_f64),
            cache_write,
            // A decimal string (`"0.010000"`).
            web_search_usd: rates
                .get("webSearchUsd")
                .and_then(Value::as_str)
                .and_then(|usd| usd.parse().ok()),
        })
    }
}

/// The Gateway's providers, in its order.
pub fn parse_providers(page: &Value) -> Vec<CatalogProvider> {
    page.get("data")
        .and_then(Value::as_array)
        .map(|providers| providers.iter().filter_map(CatalogProvider::from_json).collect())
        .unwrap_or_default()
}

/// A price per million tokens as the catalog writes it: `$3`, `$0.15`, `–` when there is none.
pub fn usd(price: Option<f64>) -> String {
    match price {
        None => "–".into(),
        Some(price) if price == 0. => "$0".into(),
        Some(price) if price >= 10. || price.fract() == 0. => format!("${price:.0}"),
        Some(price) if price >= 1. => format!("${price:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .into(),
        Some(price) => format!("${price:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .into(),
    }
}

/// A context window in tokens: `200K`, `1M`, `–`.
pub fn tokens(count: Option<u64>) -> String {
    match count {
        None => "–".into(),
        // One decimal at most, and none when it is zero: 1,048,576 is "1M" beside a 1,000,000 "1M", not "1.0M".
        Some(count) if count >= 1_000_000 => {
            let millions = format!("{:.1}", count as f64 / 1_000_000.);
            format!("{}M", millions.strip_suffix(".0").unwrap_or(&millions))
        }
        Some(count) if count >= 1_000 => format!("{}K", count / 1_000),
        Some(count) => count.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_model_with_its_prices() {
        let model = CatalogModel::from_json(&json!({
            "id": "anthropic/claude-opus-5-5", "name": "Claude Opus 5.5", "owned_by": "anthropic",
            "status": "supported", "allowed": false, "pricing": { "input": 5, "output": 25, "unit": "1M" },
            "context_window": 200000, "default_for": ["claude"]
        }))
        .expect("a model");
        assert!(model.supported && !model.allowed);
        assert_eq!((usd(model.input), usd(model.output)), ("$5".into(), "$25".into()));
        assert_eq!(tokens(model.context), "200K");
        assert_eq!(model.default_for, vec!["Claude Code".to_string()]);
    }

    #[test]
    fn writes_small_prices_and_large_windows() {
        assert_eq!(usd(Some(0.15)), "$0.15");
        assert_eq!(usd(Some(1.25)), "$1.25");
        assert_eq!(usd(None), "–");
        assert_eq!(tokens(Some(1_000_000)), "1M");
        assert_eq!(tokens(Some(1_048_576)), "1M");
        assert_eq!(tokens(Some(1_100_000)), "1.1M");
    }
}
