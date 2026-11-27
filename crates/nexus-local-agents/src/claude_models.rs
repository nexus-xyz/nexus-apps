//! Claude Code's Agent SDK initialize response contains its available models and their supported effort levels.
//! Send only initialize, extract only model metadata, then stop the process. No prompt or conversation is sent.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::models::{Effort, Model};
use crate::output;

/// Read the same model metadata as Agent SDK's `supportedModels()`, without starting a model turn.
pub async fn discover(program: &Path, path: &str) -> Option<Vec<Model>> {
    discover_with_env(program, path, None).await
}
pub(crate) async fn discover_with_env(
    program: &Path,
    path: &str,
    env: Option<&std::collections::HashMap<String, String>>,
) -> Option<Vec<Model>> {
    let mut command = output::async_command(program);
    if let Some(env) = env {
        command.env_clear().envs(env);
    }
    command
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--settings",
            "{\"disableAllHooks\":true}",
            "--tools",
            "",
        ])
        .env("PATH", path)
        .env("CLAUDE_CODE_SKIP_PROMPT_HISTORY", "1")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let initialize = json!({"type":"control_request", "request_id":"models", "request":{"subtype":"initialize"}});
        stdin.write_all(format!("{initialize}\n").as_bytes()).await.ok()?;
        stdin.flush().await.ok()?;
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await.ok()? {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("type").and_then(Value::as_str) == Some("control_response")
                && message.pointer("/response/request_id").and_then(Value::as_str) == Some("models")
            {
                return parse(&message);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let _ = child.kill().await;
    let _ = child.wait().await;
    result
}

fn parse(message: &Value) -> Option<Vec<Model>> {
    if message.pointer("/response/subtype").and_then(Value::as_str) != Some("success") {
        return None;
    }
    let entries = message.pointer("/response/response/models")?.as_array()?;
    let mut models = Vec::new();
    for entry in entries {
        let Some(id) = entry.get("value").and_then(Value::as_str).filter(|id| !id.is_empty()) else {
            continue;
        };
        if models.iter().any(|model: &Model| model.id == id) {
            continue;
        }
        let efforts = entry
            .get("supportedEffortLevels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|level| level.as_str().and_then(Effort::parse))
            .collect();
        models.push(Model {
            id: id.into(),
            name: entry.get("displayName").and_then(Value::as_str).unwrap_or(id).into(),
            efforts,
            resolved_id: entry
                .get("resolvedModel")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(String::from),
        });
    }
    Some(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_new_models_and_alias_capabilities_without_a_name_allowlist() {
        let response = json!({"response":{"subtype":"success","response":{"models":[
            {"value":"opus","resolvedModel":"claude-opus-next","displayName":"Next Opus", "supportedEffortLevels":["low","high","xhigh","max"]},
            {"value":"claude-new-family","displayName":"New family", "supportedEffortLevels":["medium"]},
            {"value":"haiku","supportsEffort":false,"resolvedModel":""},
            {"value":"opus"}, {}
        ],"account":{"ignored":"not part of the returned model catalog"}}}});
        let models = parse(&response).expect("models");
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].resolved_id.as_deref(), Some("claude-opus-next"));
        assert_eq!(
            models[0].efforts,
            [Effort::Low, Effort::High, Effort::Xhigh, Effort::Max]
        );
        assert_eq!(models[1].id, "claude-new-family");
        assert!(models[2].efforts.is_empty());
        assert!(models[2].resolved_id.is_none());
        let serialized = serde_json::to_string(&models).expect("json");
        assert!(!serialized.contains("account"));
    }

    #[test]
    fn old_and_failed_initialize_responses_leave_the_fallback_available() {
        assert_eq!(parse(&json!({"response":{"subtype":"success","response":{}}})), None);
        assert_eq!(
            parse(&json!({"response":{"subtype":"error","error":"unavailable"}})),
            None
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sends_only_initialize_and_reads_model_metadata() {
        let dir = std::env::temp_dir().join(format!("nexus-claude-models-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).expect("directory");
        let program = dir.join("claude");
        crate::output::write_fake_script(
            &program,
            r#"#!/bin/sh
read -r request
case "$request" in
  *'"subtype":"initialize"'*) ;;
  *) exit 1 ;;
esac
printf '%s\n' '{"type":"control_response","response":{"request_id":"models","subtype":"success","response":{"models":[{"value":"new-model","displayName":"New model","supportedEffortLevels":["xhigh"]}]}}}'
# Discovery must stop the process without sending a user message.
if read -r extra; then exit 1; fi
"#,
        );
        let models = discover(&program, "").await.expect("catalog");
        assert_eq!(models[0].id, "new-model");
        assert_eq!(models[0].efforts, [Effort::Xhigh]);
        std::fs::remove_dir_all(dir).expect("cleanup");
    }
}
