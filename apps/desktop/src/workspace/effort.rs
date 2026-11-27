//! Reasoning choices shared by the new and open Code composers.
use nexus_workbench::agent_models::Model;
use nexus_workbench::event::{Effort, SessionAgent};
use nexus_workbench::harness::harness;

use super::{EFFORTS, raw_id};

pub(in crate::workspace) fn options(
    agent: SessionAgent,
    models: &[Model],
    selected: Option<&str>,
) -> Vec<(Effort, &'static str)> {
    if !harness(agent).session_features().effort_selection {
        return Vec::new();
    }
    let model = match selected {
        Some(id) => models.iter().find(|model| {
            raw_id(&model.id) == raw_id(id)
                || model
                    .resolved_id
                    .as_deref()
                    .is_some_and(|resolved| raw_id(resolved) == raw_id(id))
        }),
        // Codex has no separate default entry. Use the same first model as the composer's displayed default name.
        None => models
            .iter()
            .find(|model| crate::code::is_default_entry(model))
            .or_else(|| models.first()),
    };
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
