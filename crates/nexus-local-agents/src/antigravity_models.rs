//! Antigravity's models, from `agy models`: one `id<TAB>label` line per model. The command sends no prompt and runs no
//! turn. agy names one model at several thinking levels as several ids (`gemini-3.8-flash-high`, `-medium`, `-low`);
//! its ACP session names that model once (`gemini-3.8-flash`) and the level apart (`thought_level`). So the picker lists
//! the model once, with those levels as its efforts, and the session sets both as it opens.

use std::collections::HashMap;
use std::path::Path;

use crate::models::{Effort, Model};

/// The first agy that offers models other than Gemini to an ACP client it does not know (its changelog, 1.5.1). An
/// older one lists them in `agy models`, but its ACP session refuses them.
const EVERY_MODEL_OVER_ACP: (u64, u64, u64) = (1, 5, 1);

/// None when agy did not list its models ([`crate::antigravity_models`]): it is older than 1.1.12,
/// signed out, or did not answer in time. The session then runs agy's default model.
pub async fn discover(program: &Path, path: &str, version: Option<&str>) -> Option<Vec<Model>> {
    discover_with_env(program, path, version, None).await
}
pub(crate) async fn discover_with_env(
    program: &Path,
    path: &str,
    version: Option<&str>,
    env: Option<&HashMap<String, String>>,
) -> Option<Vec<Model>> {
    let mut env = env.cloned().unwrap_or_else(|| std::env::vars().collect());
    env.insert("PATH".into(), path.into());
    let cwd = env
        .get("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let (program, asked) = (program.to_path_buf(), version.map(String::from));
    let listed = tokio::task::spawn_blocking(move || crate::antigravity_models(&program, asked.as_deref(), &env, &cwd))
        .await
        .ok()??;
    let gemini_only = version
        .and_then(crate::read_version)
        .is_none_or(|version| crate::older_than(&version, EVERY_MODEL_OVER_ACP));
    Some(fold(&listed, gemini_only))
}

/// agy's listed `(id, label)` pairs as the picker's models, in agy's order. Ids that differ only by a `-low`, `-medium`,
/// `-high` or `-xhigh` ending, two or more of them, are one model with those efforts; any other id is a model with none.
fn fold(listed: &[(String, String)], gemini_only: bool) -> Vec<Model> {
    let level = |id: &str| {
        [
            ("-low", Effort::Low),
            ("-medium", Effort::Medium),
            ("-high", Effort::High),
            ("-xhigh", Effort::Xhigh),
        ]
        .into_iter()
        .find_map(|(suffix, effort)| id.strip_suffix(suffix).map(|base| (base.to_string(), effort)))
    };
    let siblings = |base: &str| {
        listed
            .iter()
            .filter(|(id, _)| level(id).is_some_and(|(other, _)| other == base))
            .count()
    };
    let mut models: Vec<Model> = Vec::new();
    for (id, label) in listed {
        if gemini_only && !id.starts_with("gemini") {
            continue;
        }
        match level(id).filter(|(base, _)| siblings(base) >= 2) {
            Some((base, effort)) => match models.iter_mut().find(|model| model.id == base) {
                Some(model) => model.efforts.push(effort),
                None => models.push(Model {
                    id: base,
                    name: plain_name(label),
                    efforts: vec![effort],
                    resolved_id: None,
                    fast: false,
                }),
            },
            None => models.push(Model {
                id: id.clone(),
                name: plain_name(label),
                efforts: Vec::new(),
                resolved_id: None,
                fast: false,
            }),
        }
    }
    for model in &mut models {
        model.efforts.sort_by_key(|effort| match effort {
            Effort::Low => 0,
            Effort::Medium => 1,
            Effort::High => 2,
            _ => 3,
        });
    }
    // agy runs its first model when none is picked. As Claude Code's own "default" entry does, this one names it, so
    // the picker's "Default" row stands for that model and does not list it a second time.
    if let Some(first) = models.first() {
        let default = Model {
            id: "default".into(),
            name: "Default".into(),
            efforts: first.efforts.clone(),
            resolved_id: Some(first.id.clone()),
            fast: false,
        };
        models.insert(0, default);
    }
    models
}

/// agy's label without what it says of how the model runs: "Gemini 3.8 Flash (High)" is "Gemini 3.8 Flash", the
/// level being the model's effort, and "Claude Sonnet 4.6 (Thinking)" is "Claude Sonnet 4.6": agy runs the Claude
/// models with thinking always on, so it tells them apart from nothing.
fn plain_name(label: &str) -> String {
    for suffix in [" (Low)", " (Medium)", " (High)", " (XHigh)", " (Thinking)"] {
        if let Some(name) = label.strip_suffix(suffix) {
            return name.to_string();
        }
    }
    label.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed() -> Vec<(String, String)> {
        crate::read_antigravity_models(include_str!(
            "../../nexus-client/tests/fixtures/agy-1.5.8-models.stdout"
        ))
        .unwrap_or_default()
    }

    #[test]
    fn levels_of_one_model_are_its_efforts() {
        let models = fold(&listed(), false);
        let rows: Vec<(&str, &str, &[Effort])> = models
            .iter()
            .map(|model| (model.id.as_str(), model.name.as_str(), model.efforts.as_slice()))
            .collect();
        let levels = &[Effort::Low, Effort::Medium, Effort::High][..];
        assert_eq!(
            rows,
            [
                // The "Default" row: agy's first model, with its levels.
                ("default", "Default", levels),
                ("gemini-3.8-flash", "Gemini 3.8 Flash", levels),
                // Thinking is how agy always runs them, not part of the name.
                ("claude-sonnet-4-6", "Claude Sonnet 4.6", &[][..]),
                ("claude-opus-4-6-thinking", "Claude Opus 4.6", &[][..]),
                // One level only: agy's own id, as its ACP session names it.
                ("gpt-oss-120b-medium", "GPT-OSS 120B", &[][..]),
            ]
        );
        assert_eq!(models[0].resolved_id.as_deref(), Some("gemini-3.8-flash"));
    }

    #[test]
    fn extra_high_is_a_level_of_the_model_that_offers_it() {
        let listed = crate::read_antigravity_models(include_str!(
            "../../nexus-client/tests/fixtures/agy-1.5.10-models.stdout"
        ))
        .unwrap_or_default();
        let models = fold(&listed, false);
        let efforts = |id: &str| {
            models
                .iter()
                .find(|model| model.id == id)
                .map(|model| (model.name.as_str(), model.efforts.as_slice()))
        };
        let three = &[Effort::Low, Effort::Medium, Effort::High][..];
        assert_eq!(
            efforts("model-b"),
            Some((
                "Model B",
                &[Effort::Low, Effort::Medium, Effort::High, Effort::Xhigh][..]
            ))
        );
        assert_eq!(efforts("model-a"), Some(("Model A", three)));
        // agy's session names Model B once, so none of its levels is a model of its own.
        assert!(models.iter().all(|model| !model.id.starts_with("model-b-")));
    }

    #[test]
    fn older_agy_offers_gemini_only() {
        let models = fold(&listed(), true);
        assert_eq!(
            models.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(),
            ["default", "gemini-3.8-flash"]
        );
    }
}
