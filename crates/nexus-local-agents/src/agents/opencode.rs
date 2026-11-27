//! OpenCode on this machine: local sessions use the provider set in its local configuration, while Gateway launches
//! inject `OPENCODE_CONFIG_CONTENT` locked to the `nexus` provider so every model call runs through Nexus Gateway's
//! OpenAI-compatible chat completions endpoint (`<core>/v1`).

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use nexus_agents::{Agent, Opencode};
use nexus_providers::{Credential, ProviderError};
use serde_json::{Map, Value, json};

use super::{CatalogFuture, LocalAgentT, ModelFuture};
use crate::Login;
use crate::launch::{CredentialEnv, LaunchOptions, SESSION_HEADER, TEAM_HEADER, after, credential_of};

/// The environment variable OpenCode reads its Nexus key from (`{env:NEXUS_OPENCODE_API_KEY}` inside
/// `OPENCODE_CONFIG_CONTENT`), keeping the secret out of the JSON config string and out of argv.
pub const OPENCODE_KEY_ENV: &str = "NEXUS_OPENCODE_API_KEY";

/// A session keeps its native provider ID across resume. A distinct ID avoids collisions with ambient providers.
pub fn opencode_server_provider(thread_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(thread_id.as_bytes());
    let id: String = digest.iter().take(16).map(|byte| format!("{byte:02x}")).collect();
    format!("nexus_byo_{id}")
}

/// OpenCode model references reserve characters that the server may use in its model ID.
pub const OPENCODE_SERVER_MODEL: &str = "session";

/// The fallback open-weight model registered for OpenCode when the caller names none.
pub const OPENCODE_DEFAULT_MODEL: &str = "zai/glm-5.3";

const GATEWAY_STRIPPED_ENV: &[&str] = &[
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_CONTENT",
    "OPENCODE_API_KEY",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
];

impl LocalAgentT for Opencode {
    fn model_catalog<'a>(
        &self,
        _program: &'a Path,
        _env: &'a HashMap<String, String>,
        _dir: &'a Path,
    ) -> CatalogFuture<'a> {
        Box::pin(async { None })
    }

    fn models<'a>(
        &self,
        _program: &'a Path,
        _path: &'a str,
        _version: Option<&'a str>,
        _env: Option<&'a HashMap<String, String>>,
    ) -> ModelFuture<'a> {
        Box::pin(async { None })
    }

    fn settled_login(&self, _home: Option<&str>) -> Option<Login> {
        None
    }

    fn read_status(&self, _stdout: &str, _stderr: &str) -> (Login, Option<String>) {
        (Login::Unknown, None)
    }

    fn read_account(&self, _stdout: &str) -> Option<String> {
        None
    }

    fn launch(&self, options: LaunchOptions) -> (Vec<String>, HashMap<String, String>) {
        let credential = credential_of(&options);
        self.launch_with_credential(options, credential)
    }

    fn launch_with_credential(
        &self,
        options: LaunchOptions,
        credential: Credential,
    ) -> (Vec<String>, HashMap<String, String>) {
        let mut env = options.env;
        let first = super::route_env(Agent::Opencode, &credential).apply(&mut env);
        if let Credential::Gateway {
            core_url,
            session_id,
            team,
            ..
        } = &credential
        {
            env.insert(
                "OPENCODE_CONFIG_CONTENT".into(),
                gateway_config(core_url, session_id, team.as_deref(), options.model).to_string(),
            );
        }
        if let Credential::Server {
            provider_id,
            base_url,
            secret,
        } = &credential
            && let Some(model) = options.model
        {
            let mut config = json!({
                "enabled_providers": [provider_id],
                "share": "disabled",
                "autoupdate": false,
                "model": format!("{provider_id}/{OPENCODE_SERVER_MODEL}"),
                "provider": { (provider_id): {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "OpenAI-compatible server",
                    "options": { "baseURL": base_url },
                    "models": { (OPENCODE_SERVER_MODEL): { "id": model, "name": model } }
                }}
            });
            if secret.is_some() {
                config["provider"][provider_id]["options"]["apiKey"] = json!(format!("{{env:{OPENCODE_KEY_ENV}}}"));
            }
            env.insert("OPENCODE_CONFIG_CONTENT".into(), config.to_string());
        }
        (after(first, options.args), env)
    }

    /// Through Gateway, its key in a variable the injected config names; its own provider settings are removed. On its
    /// own login it keeps the provider it configured.
    fn credential_env<'c>(&self, credential: &'c Credential) -> Result<CredentialEnv<'c>, ProviderError> {
        match credential {
            Credential::Gateway { token, .. } => Ok(CredentialEnv {
                set: vec![(OPENCODE_KEY_ENV, Cow::Borrowed(token.expose()))],
                remove: GATEWAY_STRIPPED_ENV.to_vec(),
                args: Vec::new(),
            }),
            Credential::Server { secret, .. } => Ok(CredentialEnv {
                set: secret
                    .iter()
                    .map(|key| (OPENCODE_KEY_ENV, Cow::Borrowed(key.expose())))
                    .collect(),
                remove: GATEWAY_STRIPPED_ENV.iter().copied().chain([OPENCODE_KEY_ENV]).collect(),
                args: Vec::new(),
            }),
            Credential::AgentLogin { .. } => Ok(CredentialEnv {
                set: super::account_dir_env(Agent::Opencode, credential)
                    .into_iter()
                    .collect(),
                remove: Vec::new(),
                args: Vec::new(),
            }),
            other => Err(ProviderError::NotOffered {
                agent: Agent::Opencode,
                kind: other.kind(),
            }),
        }
    }

    fn shell_variable(&self, _name: &str, _value: &str) -> Option<[String; 2]> {
        None
    }

    fn takes_short_model_flags(&self) -> bool {
        false
    }

    fn provider_model_id(&self, id: &str) -> String {
        id.strip_prefix("nexus/").unwrap_or(id).to_string()
    }

    fn takes_instructions(&self) -> bool {
        false
    }

    fn with_instructions(&self, args: &[String], _text: &str) -> Vec<String> {
        args.to_vec()
    }

    fn takes_recording(&self) -> bool {
        false
    }

    fn takes_prompt_argument(&self) -> bool {
        false
    }

    fn with_recording(&self, args: &[String], _prompt: &str, _sheet: &Path) -> Vec<String> {
        args.to_vec()
    }

    fn memory(&self, _home: &Path, _repo: &Path) -> std::io::Result<Vec<nexus_agent_memory::NativeEntry>> {
        crate::native_memory::unsupported()
    }

    fn write_memory(&self, _home: &Path, _repo: &Path, _entries: &[nexus_agent_memory::Entry]) -> std::io::Result<()> {
        crate::native_memory::unsupported()
    }

    fn keeps_transcript(&self) -> bool {
        false
    }

    fn newest_conversation(&self, _home: &Path, _cwd: &Path, _since: SystemTime) -> Option<String> {
        None
    }

    fn transcript(&self, _home: &Path, _cwd: &Path, _id: &str) -> Option<PathBuf> {
        None
    }
}

/// The config a Gateway launch injects (`OPENCODE_CONFIG_CONTENT`): only the `nexus` provider, at Gateway's
/// OpenAI-compatible endpoint, with the session's headers and the model it runs.
fn gateway_config(core: &str, session_id: &str, team: Option<&str>, model: Option<&str>) -> Value {
    let selected = model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(|model| model.strip_prefix("nexus/").unwrap_or(model))
        .unwrap_or(OPENCODE_DEFAULT_MODEL);

    let mut headers = Map::new();
    headers.insert(SESSION_HEADER.into(), Value::String(session_id.into()));
    if let Some(team) = team.filter(|team| !team.is_empty()) {
        headers.insert(TEAM_HEADER.into(), Value::String(team.into()));
    }

    let mut models = Map::new();
    models.insert(selected.into(), json!({ "name": selected }));
    if selected != OPENCODE_DEFAULT_MODEL {
        models.insert(OPENCODE_DEFAULT_MODEL.into(), json!({ "name": OPENCODE_DEFAULT_MODEL }));
    }

    json!({
        "$schema": "https://opencode.ai/config.json",
        "enabled_providers": ["nexus"],
        "share": "disabled",
        "autoupdate": false,
        "model": format!("nexus/{selected}"),
        "provider": {
            "nexus": {
                "npm": "@ai-sdk/openai-compatible",
                "name": "Nexus Gateway",
                "options": {
                    "baseURL": format!("{core}/v1"),
                    "apiKey": format!("{{env:{OPENCODE_KEY_ENV}}}"),
                    "headers": headers,
                },
                "models": models,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::Route;

    #[test]
    fn opencode_gateway_launch_locks_provider_to_nexus_and_strips_overrides() {
        let (args, env) = Opencode.launch(LaunchOptions {
            route: Route::Gateway,
            core_url: "https://gateway.invalid/",
            api_key: "nk_test_secret",
            session_id: "sess_123",
            team: Some("platform"),
            model: Some("deepseek/deepseek-v4.1-flash-1017"),
            model_catalog: None,
            web_search: false,
            args: vec!["acp".into()],
            env: HashMap::from([
                ("OPENCODE_API_KEY".into(), "leaked".into()),
                ("OPENCODE_CONFIG".into(), "/tmp/override.json".into()),
                ("OPENCODE_CONFIG_CONTENT".into(), "{}".into()),
                ("OPENAI_API_KEY".into(), "sk-leaked".into()),
                ("ANTHROPIC_API_KEY".into(), "sk-ant-leaked".into()),
                ("HOME".into(), "/work".into()),
            ]),
        });
        assert_eq!(args, ["acp"]);
        assert_eq!(env.get("HOME").map(String::as_str), Some("/work"));
        assert_eq!(env.get(OPENCODE_KEY_ENV).map(String::as_str), Some("nk_test_secret"));
        for stripped in [
            "OPENCODE_API_KEY",
            "OPENCODE_CONFIG",
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
        ] {
            assert!(!env.contains_key(stripped), "{stripped} must be stripped");
        }
        let raw = env.get("OPENCODE_CONFIG_CONTENT").expect("OPENCODE_CONFIG_CONTENT set");
        assert!(!raw.contains("nk_test_secret"), "secret stays out of config JSON");
        let parsed: Value = serde_json::from_str(raw).expect("valid config json");
        assert_eq!(parsed["enabled_providers"], json!(["nexus"]));
        assert_eq!(parsed["share"], json!("disabled"));
        assert_eq!(parsed["autoupdate"], json!(false));
        // No MCP server: one added here must also get the session's fence.
        assert!(parsed.get("mcp").is_none(), "{parsed}");
        assert_eq!(parsed["model"], json!("nexus/deepseek/deepseek-v4.1-flash-1017"));
        assert_eq!(
            parsed["provider"]["nexus"]["options"]["baseURL"],
            json!("https://gateway.invalid/v1")
        );
        assert_eq!(
            parsed["provider"]["nexus"]["options"]["apiKey"],
            json!("{env:NEXUS_OPENCODE_API_KEY}")
        );
        assert_eq!(
            parsed["provider"]["nexus"]["options"]["headers"],
            json!({
                "x-nexus-session": "sess_123",
                "x-nexus-team": "platform",
            })
        );
        assert!(parsed["provider"]["nexus"]["models"]["deepseek/deepseek-v4.1-flash-1017"].is_object());
        assert!(parsed["provider"]["nexus"]["models"]["zai/glm-5.3"].is_object());
    }

    #[test]
    fn opencode_gateway_launch_uses_default_model_when_none_is_given() {
        let (_, env) = Opencode.launch(LaunchOptions {
            route: Route::Gateway,
            core_url: "https://gateway.invalid",
            api_key: "nk_test",
            session_id: "sess_default",
            team: None,
            model: None,
            model_catalog: None,
            web_search: false,
            args: vec!["acp".into()],
            env: HashMap::new(),
        });
        let parsed: Value = serde_json::from_str(env.get("OPENCODE_CONFIG_CONTENT").expect("config")).expect("json");
        assert_eq!(parsed["model"], json!("nexus/zai/glm-5.3"));
        assert_eq!(
            parsed["provider"]["nexus"]["options"]["headers"],
            json!({ "x-nexus-session": "sess_default" })
        );
    }

    #[test]
    fn opencode_subscription_launch_leaves_local_env_untouched() {
        let (args, env) = Opencode.launch(LaunchOptions {
            route: Route::Subscription,
            core_url: "https://gateway.invalid",
            api_key: "nk_test",
            session_id: "sess_local",
            team: None,
            model: None,
            model_catalog: None,
            web_search: false,
            args: vec!["acp".into()],
            env: HashMap::from([("OPENCODE_CONFIG".into(), "/home/me/opencode.json".into())]),
        });
        assert_eq!(args, ["acp"]);
        assert_eq!(
            env.get("OPENCODE_CONFIG").map(String::as_str),
            Some("/home/me/opencode.json")
        );
        assert!(!env.contains_key(OPENCODE_KEY_ENV));
    }

    #[test]
    fn opencode_provider_model_id_strips_nexus_prefix() {
        assert_eq!(Opencode.provider_model_id("zai/glm-5.3"), "zai/glm-5.3");
        assert_eq!(Opencode.provider_model_id("nexus/zai/glm-5.3"), "zai/glm-5.3");
    }
}
