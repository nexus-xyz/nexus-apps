//! Reasoning and speed choices shared by the new and open Code composers.
use nexus_workbench::agent_models::Model;
use nexus_workbench::event::{Effort, SessionAgent, Via};
use nexus_workbench::harness::harness;

use super::{EFFORTS, raw_id};

/// The catalog's entry for `selected`, by its id or the id it resolves to. For None, the Default entry, else the first:
/// Codex has no Default entry, and the composer names its first model.
fn entry<'a>(models: &'a [Model], selected: Option<&str>) -> Option<&'a Model> {
    match selected {
        Some(id) => models.iter().find(|model| {
            raw_id(&model.id) == raw_id(id)
                || model
                    .resolved_id
                    .as_deref()
                    .is_some_and(|resolved| raw_id(resolved) == raw_id(id))
        }),
        None => models
            .iter()
            .find(|model| crate::code::is_default_entry(model))
            .or_else(|| models.first()),
    }
}

/// Whether `selected` (None: what Default runs) has a fast speed in a session of `agent` paid `via`
/// (`driver::fast_allowed`). Claude Code reports no speeds, so its fast models are a list (`claude::supports_fast`);
/// Codex reports a `priority` tier per model.
pub(in crate::workspace) fn has_fast(agent: SessionAgent, via: Via, models: &[Model], selected: Option<&str>) -> bool {
    if !nexus_workbench::driver::fast_allowed(agent, via) {
        return false;
    }
    let entry = entry(models, selected);
    match agent {
        SessionAgent::Claude => [selected, entry.and_then(|entry| entry.resolved_id.as_deref())]
            .into_iter()
            .flatten()
            .any(nexus_workbench::claude::supports_fast),
        _ => entry.is_some_and(|entry| entry.fast),
    }
}

pub(in crate::workspace) fn options(
    agent: SessionAgent,
    models: &[Model],
    selected: Option<&str>,
) -> Vec<(Effort, &'static str)> {
    if !harness(agent).session_features().effort_selection {
        return Vec::new();
    }
    let model = entry(models, selected);
    EFFORTS
        .iter()
        .copied()
        .filter(|(effort, _)| match model {
            Some(model) => model.efforts.contains(effort),
            None => harness(agent).usual_efforts().contains(effort),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, efforts: &[Effort]) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            efforts: efforts.to_vec(),
            resolved_id: None,
            fast: false,
        }
    }

    #[test]
    fn codex_default_exposes_the_displayed_models_efforts() {
        let models = [model("gpt-6.1", &[Effort::Low, Effort::Ultra])];
        assert_eq!(
            options(SessionAgent::Codex, &models, None),
            [(Effort::Low, "Low"), (Effort::Ultra, "Ultra")]
        );
        assert_eq!(
            options(SessionAgent::Codex, &models, Some("openai/gpt-6.1")),
            options(SessionAgent::Codex, &models, None)
        );
    }

    #[test]
    fn a_known_model_without_reasoning_does_not_inherit_other_models_efforts() {
        let models = [model("reasoner", &[Effort::High]), model("plain", &[])];
        assert!(options(SessionAgent::Codex, &models, Some("plain")).is_empty());
        assert!(!options(SessionAgent::Codex, &[], None).is_empty());
        assert!(options(SessionAgent::Nexus, &[], None).is_empty());
        // agy's thinking levels, and no other.
        assert_eq!(
            options(SessionAgent::Antigravity, &[], None)
                .into_iter()
                .map(|(effort, _)| effort)
                .collect::<Vec<_>>(),
            [Effort::Low, Effort::Medium, Effort::High]
        );
    }

    #[test]
    fn cursor_offers_no_effort_even_with_model_levels() {
        let models = [model("reasoner", &[Effort::High])];
        assert!(options(SessionAgent::Cursor, &models, None).is_empty());
        assert!(options(SessionAgent::Cursor, &[], None).is_empty());
    }

    #[test]
    fn fast_is_claudes_listed_models_and_codexs_priority_tier_on_its_plan() {
        let mut sol = model("gpt-6-sol", &[]);
        sol.fast = true;
        let codex = [sol, model("gpt-6-mini", &[])];
        let plan = Via::Subscription;
        assert!(has_fast(SessionAgent::Codex, plan, &codex, Some("gpt-6-sol")));
        assert!(
            has_fast(SessionAgent::Codex, plan, &codex, None),
            "Default is the first model"
        );
        assert!(
            !has_fast(SessionAgent::Codex, plan, &codex, Some("gpt-6-mini")),
            "no priority tier"
        );
        assert!(
            !has_fast(SessionAgent::Codex, Via::Nexus, &codex, Some("gpt-6-sol")),
            "Gateway has no price for the tier"
        );

        let mut default = model("default", &[]);
        default.resolved_id = Some("claude-opus-5-5".into());
        let mut opus = model("opus", &[]);
        opus.resolved_id = Some("claude-opus-5-5".into());
        let claude = [default, opus, model("haiku", &[])];
        for via in [Via::Nexus, Via::Subscription] {
            assert!(has_fast(SessionAgent::Claude, via, &claude, None));
            assert!(
                has_fast(SessionAgent::Claude, via, &claude, Some("opus")),
                "an alias, by what it resolves to"
            );
            assert!(has_fast(
                SessionAgent::Claude,
                via,
                &claude,
                Some("anthropic/claude-opus-5.5")
            ));
            assert!(!has_fast(SessionAgent::Claude, via, &claude, Some("haiku")));
        }
        assert!(!has_fast(SessionAgent::Cursor, plan, &codex, Some("gpt-6-sol")));
    }

    #[test]
    fn default_entries_and_resolved_aliases_keep_their_reported_levels() {
        let mut default = model("default", &[Effort::Medium]);
        default.resolved_id = Some("resolved-model".into());
        let models = [model("first", &[Effort::Low]), default];
        assert_eq!(
            options(SessionAgent::Claude, &models, None),
            [(Effort::Medium, "Medium")]
        );
        assert_eq!(
            options(SessionAgent::Claude, &models, Some("maker/resolved-model")),
            [(Effort::Medium, "Medium")]
        );
    }
}
