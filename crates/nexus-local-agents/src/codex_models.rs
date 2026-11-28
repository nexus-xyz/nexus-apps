//! Model capabilities from the installed Codex binary, refreshed without starting an inference turn.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::models::{Effort, Model};
use crate::output;

/// Codex's service tier for its fast speed, as `turn/start`'s `serviceTier` takes it.
pub const FAST_TIER: &str = "priority";

/// None on older binaries, invalid output or a timeout: callers retain the Gateway policy as their fallback.
pub async fn discover(program: &Path, path: &str) -> Option<Vec<Model>> {
    discover_with_env(program, path, None).await
}
pub(crate) async fn discover_with_env(
    program: &Path,
    path: &str,
    env: Option<&std::collections::HashMap<String, String>>,
) -> Option<Vec<Model>> {
    // Refresh first so new models appear without a desktop release. Offline clients use Codex's bundled catalog.
    for (args, deadline) in [
        (vec!["debug", "models"], Duration::from_secs(3)),
        (vec!["debug", "models", "--bundled"], Duration::from_secs(2)),
    ] {
        let mut command = output::async_command(program);
        if let Some(env) = env {
            command.env_clear().envs(env);
        }
        let output = command
            .args(args)
            .env("PATH", path)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        if let Ok(Ok(output)) = tokio::time::timeout(deadline, output).await
            && output.status.success()
            && let Some(models) = parse(&String::from_utf8_lossy(&output.stdout))
        {
            return Some(models);
        }
    }
    None
}

fn parse(text: &str) -> Option<Vec<Model>> {
    let catalog: Value = serde_json::from_str(text).ok()?;
    let mut entries: Vec<_> = catalog.get("models")?.as_array()?.iter().collect();
    entries.sort_by_key(|entry| entry.get("priority").and_then(Value::as_u64).unwrap_or(u64::MAX));
    let mut models = Vec::new();
    for entry in entries {
        if entry.get("visibility").and_then(Value::as_str) != Some("list") {
            continue;
        }
        let Some(id) = entry.get("slug").and_then(Value::as_str).filter(|id| !id.is_empty()) else {
            continue;
        };
        if models.iter().any(|model: &Model| model.id == id) {
            continue;
        }
        let efforts = entry
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|level| level.get("effort").and_then(Value::as_str).and_then(Effort::parse))
            .collect();
        // Codex 0.171 names its fast speed `priority` ("Fast") in `service_tiers`.
        let fast = entry
            .get("service_tiers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|tier| tier.get("id").and_then(Value::as_str) == Some(FAST_TIER));
        models.push(Model {
            id: id.into(),
            name: entry.get("display_name").and_then(Value::as_str).unwrap_or(id).into(),
            efforts,
            resolved_id: None,
            fast,
        });
    }
    Some(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_visible_models_in_priority_order_with_their_own_efforts() {
        let models = parse(
            r#"{"models":[
            {"slug":"gpt-6-astra","display_name":"GPT-6 Astra","visibility":"list","priority":3,
             "supported_reasoning_levels":[{"effort":"xhigh"},{"effort":"max"},{"effort":"ultra"}],
             "service_tiers":[{"id":"priority","name":"Fast","description":"2x speed, increased usage"}]},
            {"slug":"hidden","visibility":"hide","priority":0},
            {"slug":"gpt-6.1-sol","visibility":"list","priority":1,
             "supported_reasoning_levels":[{"effort":"low"},{"effort":"ultra"}]},
            {"slug":"gpt-6.1-terra","display_name":"GPT-6.1 Terra","visibility":"list","priority":2,
             "supported_reasoning_levels":[{"effort":"low"},{"effort":"ultra"}]},
            {"slug":"gpt-future-release","visibility":"list","priority":4,
             "supported_reasoning_levels":[{"effort":"none"},{"effort":"minimal"}]},
            {"slug":"gpt-6.1-sol","visibility":"list","priority":5},
            {"visibility":"list"}
        ]}"#,
        )
        .expect("catalog");
        assert_eq!(
            models.iter().map(|model| model.id.as_str()).collect::<Vec<_>>(),
            ["gpt-6.1-sol", "gpt-6.1-terra", "gpt-6-astra", "gpt-future-release"]
        );
        assert_eq!(models[0].name, "gpt-6.1-sol");
        assert_eq!(models[0].efforts, [Effort::Low, Effort::Ultra]);
        assert_eq!(models[1].name, "GPT-6.1 Terra");
        assert_eq!(models[2].efforts, [Effort::Xhigh, Effort::Max, Effort::Ultra]);
        assert_eq!(models[3].efforts, [Effort::Disabled, Effort::Minimal]);
        // Only a model with a `priority` tier is fast.
        assert_eq!(
            models.iter().map(|model| model.fast).collect::<Vec<_>>(),
            [false, false, true, false]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn falls_back_to_the_bundled_catalog_when_refresh_fails() {
        let dir = std::env::temp_dir().join(format!("nexus-codex-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).expect("directory");
        let program = dir.join("codex");
        crate::output::write_fake_script(
            &program,
            r#"#!/bin/sh
if [ "$3" != "--bundled" ]; then exit 1; fi
printf '%s' '{"models":[{"slug":"gpt-6-astra","visibility":"list","supported_reasoning_levels":[{"effort":"ultra"}]}]}'
"#,
        );
        let models = discover(&program, "").await.expect("bundled catalog");
        assert_eq!(models[0].id, "gpt-6-astra");
        assert_eq!(models[0].efforts, [Effort::Ultra]);
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn rejects_non_catalog_output_and_preserves_an_empty_catalog() {
        assert_eq!(parse("old binary help output"), None);
        assert_eq!(parse(r#"{"models":null}"#), None);
        assert_eq!(parse(r#"{"models":[]}"#), Some(vec![]));
    }
}
