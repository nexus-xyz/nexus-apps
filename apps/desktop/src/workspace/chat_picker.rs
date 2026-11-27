//! Chat model picker rules shared by rendering, activation, and transition tests.

use std::collections::HashSet;

use super::model_menu::MenuRow;
use crate::model::{AUTO, ModelChoice, maker_name, older_models};

/// Listed models in display order: usable first, then coming soon, without policy-off entries or Auto.
fn listed(models: &[ModelChoice]) -> impl Iterator<Item = (usize, &ModelChoice)> {
    let group = move |usable| {
        models
            .iter()
            .enumerate()
            .filter(move |(_, model)| !model.off && model.id != AUTO && model.usable == usable)
    };
    group(true).chain(group(false))
}

pub(super) fn makers(models: &[ModelChoice]) -> Vec<String> {
    let mut seen = HashSet::new();
    listed(models)
        .map(|(_, model)| model.maker.clone())
        .filter(|maker| seen.insert(maker.clone()))
        .collect()
}

/// A removed maker cannot leave the rail and list in different sections after a catalog refresh.
pub(super) fn maker(models: &[ModelChoice], remembered: Option<&str>, picked: Option<&str>) -> Option<String> {
    let makers = makers(models);
    remembered
        .filter(|maker| makers.iter().any(|listed| listed == maker))
        .map(str::to_owned)
        .or_else(|| {
            listed(models)
                .find(|(_, model)| Some(model.id.as_str()) == picked)
                .map(|(_, model)| model.maker.clone())
        })
        .or_else(|| makers.into_iter().next())
}

pub(super) fn rows(
    models: &[ModelChoice],
    query: &str,
    maker: Option<&str>,
    picked: Option<&str>,
    older_open: bool,
) -> Vec<MenuRow> {
    let query = query.trim().to_lowercase();
    if !query.is_empty() {
        let mut rows = Vec::new();
        if "auto".contains(&query) || "nexus".contains(&query) {
            rows.push(MenuRow::Auto);
        }
        rows.extend(
            listed(models)
                .filter(|(_, model)| {
                    [&model.name, &model.id, &model.maker, &maker_name(&model.maker)]
                        .iter()
                        .any(|field| field.to_lowercase().contains(&query))
                })
                .map(|(index, _)| MenuRow::Model(index)),
        );
        return rows;
    }
    // A policy-off newer version cannot fold the only allowed version of its line.
    let visible: Vec<_> = listed(models).map(|(_, model)| model.clone()).collect();
    let older = older_models(&visible);
    let listed: Vec<_> = listed(models)
        .filter(|(_, model)| Some(model.maker.as_str()) == maker)
        .collect();
    let is_older = |model: &ModelChoice| older.contains(&model.id) && Some(model.id.as_str()) != picked;
    let mut rows = vec![MenuRow::Auto];
    rows.extend(
        listed
            .iter()
            .filter(|(_, model)| !is_older(model))
            .map(|(index, _)| MenuRow::Model(*index)),
    );
    let rest: Vec<_> = listed
        .iter()
        .filter(|(_, model)| is_older(model))
        .map(|(index, _)| *index)
        .collect();
    if !rest.is_empty() {
        rows.push(MenuRow::Older(rest.len()));
        if older_open {
            rows.extend(rest.into_iter().map(MenuRow::Model));
        }
    }
    rows
}

/// Resolve the current catalog entry by stable identity before committing a choice.
pub(super) fn pickable<'a>(models: &'a [ModelChoice], id: &str) -> Option<&'a ModelChoice> {
    models
        .iter()
        .find(|model| model.id == id && model.id != AUTO && model.usable && !model.off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, name: &str, maker: &str, usable: bool, off: bool) -> ModelChoice {
        ModelChoice {
            id: id.into(),
            name: name.into(),
            maker: maker.into(),
            hint: None,
            usable,
            off,
            output_usd: None,
        }
    }

    fn catalog() -> Vec<ModelChoice> {
        vec![
            model("anthropic/claude-opus-4-5", "Opus 4.5", "anthropic", true, false),
            model("anthropic/claude-opus-5-5", "Opus 5.5", "anthropic", true, false),
            model("openai/gpt-5", "GPT 5", "openai", true, false),
            model("openai/gpt-6", "GPT 6", "openai", false, false),
            model("google/gemini-3", "Gemini 3", "google", false, true),
            model(AUTO, "Auto", "nexus", true, false),
        ]
    }

    #[test]
    fn query_matches_name_id_and_maker_without_case_or_edge_spaces() {
        let models = catalog();
        for (query, expected) in [
            (" OPUS 5.5 ", vec![MenuRow::Model(1)]),
            ("anthropic/claude-opus-4-5", vec![MenuRow::Model(0)]),
            ("Anthropic", vec![MenuRow::Model(0), MenuRow::Model(1)]),
            ("OPENAI", vec![MenuRow::Model(2), MenuRow::Model(3)]),
        ] {
            assert_eq!(rows(&models, query, Some("openai"), None, false), expected, "{query}");
        }
        assert!(rows(&models, "no matching model", Some("anthropic"), None, false).is_empty());
    }

    #[test]
    fn clearing_search_returns_to_the_remembered_maker() {
        let models = catalog();
        let remembered = maker(&models, Some("openai"), None);
        assert_eq!(
            rows(&models, "opus", remembered.as_deref(), None, false),
            vec![MenuRow::Model(0), MenuRow::Model(1)]
        );
        for query in ["", " \t "] {
            assert_eq!(
                rows(&models, query, remembered.as_deref(), None, false),
                vec![MenuRow::Auto, MenuRow::Model(2), MenuRow::Model(3)]
            );
        }
        // A rail click clears search and replaces the remembered maker.
        let remembered = maker(&models, Some("anthropic"), None);
        assert_eq!(
            rows(&models, "", remembered.as_deref(), None, false),
            vec![MenuRow::Auto, MenuRow::Model(1), MenuRow::Older(1)]
        );
    }

    #[test]
    fn auto_is_pinned_once_and_searchable_by_its_name_or_nexus() {
        let models = catalog();
        assert_eq!(rows(&models, "auto", None, None, false), vec![MenuRow::Auto]);
        assert_eq!(rows(&models, "NEXUS", None, None, false), vec![MenuRow::Auto]);
        assert_eq!(rows(&models, "", None, None, false), vec![MenuRow::Auto]);
        assert!(pickable(&models, AUTO).is_none(), "Auto uses its dedicated action");
    }

    #[test]
    fn policy_off_entries_are_absent_and_coming_soon_entries_are_not_pickable() {
        let models = catalog();
        assert_eq!(makers(&models), ["anthropic", "openai"]);
        assert!(rows(&models, "gemini", None, None, true).is_empty());
        assert_eq!(rows(&models, "gpt-6", None, None, false), vec![MenuRow::Model(3)]);
        assert!(pickable(&models, "google/gemini-3").is_none());
        assert!(pickable(&models, "openai/gpt-6").is_none());
        assert!(pickable(&models, "openai/gpt-5").is_some());
    }

    #[test]
    fn older_models_expand_and_the_selected_older_model_stays_visible() {
        let models = catalog();
        assert_eq!(
            rows(&models, "", Some("anthropic"), None, false),
            vec![MenuRow::Auto, MenuRow::Model(1), MenuRow::Older(1)]
        );
        assert_eq!(
            rows(&models, "", Some("anthropic"), None, true),
            vec![MenuRow::Auto, MenuRow::Model(1), MenuRow::Older(1), MenuRow::Model(0)]
        );
        assert_eq!(
            rows(&models, "", Some("anthropic"), Some("anthropic/claude-opus-4-5"), false),
            vec![MenuRow::Auto, MenuRow::Model(0), MenuRow::Model(1)]
        );
        assert_eq!(
            rows(&models, "4.5", Some("anthropic"), None, false),
            vec![MenuRow::Model(0)],
            "search bypasses the fold"
        );
    }

    #[test]
    fn catalog_refresh_resolves_makers_and_selection_against_current_data() {
        let mut models = catalog();
        assert_eq!(
            maker(&models, Some("anthropic"), Some("openai/gpt-5")).as_deref(),
            Some("anthropic")
        );
        models.retain(|model| model.maker != "anthropic");
        assert_eq!(
            maker(&models, Some("anthropic"), Some("openai/gpt-5")).as_deref(),
            Some("openai")
        );
        assert!(pickable(&models, "anthropic/claude-opus-5-5").is_none());
        models.iter_mut().find(|model| model.id == "openai/gpt-5").unwrap().off = true;
        assert!(
            pickable(&models, "openai/gpt-5").is_none(),
            "even stale usable=true cannot bypass policy"
        );
        models.clear();
        assert_eq!(maker(&models, Some("openai"), Some("openai/gpt-5")), None);
        assert_eq!(rows(&models, "", None, None, false), [MenuRow::Auto]);
    }

    #[test]
    fn maker_fallback_uses_visible_selection_then_first_listed_maker() {
        let models = catalog();
        assert_eq!(maker(&models, None, Some("openai/gpt-5")).as_deref(), Some("openai"));
        assert_eq!(maker(&models, Some("removed"), None).as_deref(), Some("anthropic"));
        assert_eq!(
            maker(&models, None, Some("google/gemini-3")).as_deref(),
            Some("anthropic")
        );
    }

    #[test]
    fn a_policy_off_newer_version_does_not_hide_the_allowed_version() {
        let mut models = catalog();
        models[1].off = true;
        assert_eq!(
            rows(&models, "", Some("anthropic"), None, false),
            vec![MenuRow::Auto, MenuRow::Model(0)]
        );
    }
}
