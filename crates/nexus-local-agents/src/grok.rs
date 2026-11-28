//! Subscription preflight shared by the native Grok launcher and the ACP workbench.
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

/// Grok still accepts third-party endpoints when API key sign-in is disabled.
/// See <https://docs.x.ai/build/enterprise> and <https://docs.x.ai/build/settings>.
pub fn subscription_env(env: &mut HashMap<String, String>) {
    env.retain(|key, _| {
        let name = key.to_ascii_uppercase();
        !matches!(
            name.as_str(),
            "XAI_API_KEY"
                | "GROK_CODE_XAI_API_KEY"
                | "GROK_ALPHA_TEST_KEY"
                | "GROK_LEADER_SOCKET"
                | "GROK_CLI_CHAT_PROXY_BASE_URL"
                | "GROK_DISABLE_API_KEY_AUTH"
        )
    });
    env.insert("GROK_DISABLE_API_KEY_AUTH".into(), "true".into());
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inspection {
    login_policy: Policy,
    config_sources: Sources,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Policy {
    api_key_auth_disabled: bool,
}

#[derive(Deserialize)]
struct Sources {
    layers: Vec<Layer>,
}

#[derive(Deserialize)]
struct Layer {
    path: std::path::PathBuf,
}

// Check every configured model. A later model selection or a tool can use a different model.
fn overrides_subscription(config: &toml::Table) -> bool {
    fn has(table: &toml::Value, names: &[&str]) -> bool {
        names.iter().any(|name| table.get(*name).is_some())
    }
    if config
        .get("model")
        .and_then(toml::Value::as_table)
        .is_some_and(|models| {
            models.values().any(|model| {
                has(
                    model,
                    &[
                        "api_key",
                        "env_key",
                        "base_url",
                        "api_base_url",
                        "auth_provider",
                        "extra_headers",
                        "env_http_headers",
                        "model_provider",
                        "mtls_cert_dir",
                        "query_params",
                    ],
                )
            })
        })
    {
        return true;
    }
    if ["auth", "grok_com_config"]
        .iter()
        .filter_map(|name| config.get(*name))
        .any(|auth| {
            has(
                auth,
                &[
                    "oidc",
                    "oauth2",
                    "preferred_method",
                    "auth_provider_command",
                    "force_login_team_uuid",
                    "grok_ws_url",
                    "grok_ws_origin",
                ],
            ) || auth.get("force_cursor_sign_in").and_then(toml::Value::as_bool) == Some(true)
        })
        || config.get("endpoints").is_some_and(|endpoints| {
            has(
                endpoints,
                &[
                    "cli_chat_proxy_base_url",
                    "xai_api_base_url",
                    "models_base_url",
                    "models_list_url",
                    "models_endpoint",
                    "deployment_key",
                    "managed_config_url",
                ],
            )
        })
    {
        return true;
    }
    // Grok flattens settings into each version, campaign or experiment patch, including experiment arms.
    config.get("arms").and_then(toml::Value::as_table).is_some_and(|arms| {
        arms.values()
            .filter_map(toml::Value::as_table)
            .any(overrides_subscription)
    }) || ["version_overrides", "campaigns", "experiments"]
        .iter()
        .filter_map(|name| config.get(*name).and_then(toml::Value::as_array))
        .flatten()
        .filter_map(toml::Value::as_table)
        .any(overrides_subscription)
}

const UNVERIFIED: &str = "Could not verify Grok's subscription settings. Check `grok inspect --json` and update Grok.";

/// Inspect the same executable, effective config, environment and directory that the session will use.
/// Only config files named by Grok are inspected. Parser errors can include secret text, so discard them.
pub fn check_subscription(program: &Path, cwd: &Path, env: &HashMap<String, String>) -> Result<(), String> {
    for (name, value) in env {
        let name = name.to_ascii_uppercase();
        if !value.is_empty()
            && (name.starts_with("GROK_OIDC_")
                || name.starts_with("GROK_OAUTH2_")
                || matches!(
                    name.as_str(),
                    "GROK_AUTH"
                        | "GROK_AUTH_PATH"
                        | "GROK_AUTH_PROVIDER_COMMAND"
                        | "GROK_FORCE_LOGIN_TEAM_ID"
                        | "GROK_CONFIG"
                        | "GROK_MANAGED_CONFIG"
                        | "GROK_MANAGED_CONFIG_URL"
                        | "GROK_SETTINGS_CACHE"
                        | "GROK_DEPLOYMENT_KEY"
                        | "GROK_WS_URL"
                        | "GROK_WS_ORIGIN"
                        | "XAI_API_BASE_URL"
                        | "GROK_XAI_API_BASE_URL"
                        | "GROK_CLI_CHAT_PROXY_BASE_URL"
                        | "GROK_MODELS_BASE_URL"
                        | "GROK_MODELS_LIST_URL"
                ))
        {
            return Err(format!(
                "Grok has a credential or endpoint override ({name}). Use a profile with your personal Grok sign-in."
            ));
        }
    }
    let mut cmd = crate::command(program);
    cmd.args(["inspect", "--json"]).current_dir(cwd).env_clear().envs(env);
    let (status, stdout) = crate::output::stdout_within(cmd, Duration::from_secs(10)).ok_or(UNVERIFIED)?;
    let inspect: Inspection = serde_json::from_str(&stdout).map_err(|_| UNVERIFIED)?;
    if !status.success() || !inspect.login_policy.api_key_auth_disabled {
        return Err(UNVERIFIED.into());
    }
    for layer in inspect.config_sources.layers {
        if !layer.path.is_absolute() || layer.path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            return Err(UNVERIFIED.into());
        }
        let config = std::fs::read_to_string(layer.path).map_err(|_| UNVERIFIED)?;
        let config = toml::from_str::<toml::Table>(&config).map_err(|_| UNVERIFIED)?;
        if overrides_subscription(&config) {
            return Err("Grok has a custom model, credential or endpoint configured. Use a Grok profile without these overrides to run on your subscription.".into());
        }
    }
    Ok(())
}

/// Build only local conversation arguments. Terminate option parsing before positional prompt text
/// so it cannot dispatch a vendor subcommand (for example, `agent serve` or `worker`).
pub fn terminal_args(args: &[String]) -> Result<Vec<String>, String> {
    let mut result = vec!["--no-leader".into(), "--permission-mode".into(), "default".into()];
    let mut prompts = Vec::new();
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        let (flag, inline) = arg.split_once('=').map_or((arg.as_str(), None), |(f, v)| (f, Some(v)));
        match flag {
            "--" => {
                prompts.extend(args.cloned());
                break;
            }
            "--resume" | "-r" if inline.is_none() && args.peek().is_none_or(|arg| arg.starts_with('-')) => {
                result.push("--resume".into());
            }
            "--model" | "-m" | "--resume" | "-r" | "--session-id" | "-s" | "--reasoning-effort" | "--effort"
            | "--output-format" => {
                let value = match inline {
                    Some(value) => value,
                    None => args
                        .next()
                        .map(String::as_str)
                        .ok_or("Supply a value for the Grok option.")?,
                };
                if value.is_empty() || value.starts_with('-') {
                    return Err("Supply a value for the Grok option.".into());
                }
                let name = match flag {
                    "-r" => "--resume",
                    "-s" => "--session-id",
                    "-m" => "--model",
                    _ => flag,
                };
                result.extend([name.to_owned(), value.to_owned()]);
            }
            "-p" | "--single" => {
                result.push("--single".into());
                if let Some(value) = inline {
                    prompts.push(value.to_owned());
                } else if let Some(value) = args.next_if(|value| !value.starts_with('-')) {
                    prompts.push(value.clone());
                }
            }
            "-c" | "--continue" | "--no-alt-screen" | "--fullscreen" | "--no-memory" if inline.is_none() => {
                result.push(arg.clone())
            }
            _ if !arg.starts_with('-') => prompts.push(arg.clone()),
            _ => return Err("This Grok option is unavailable on the local subscription launcher.".into()),
        }
    }
    if !prompts.is_empty() {
        result.push("--".into());
        result.extend(prompts);
    }
    Ok(result)
}
