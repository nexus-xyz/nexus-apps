//! Antigravity (`agy`) on this machine: its settings file and `/usage` read, and its launch, which runs only on the
//! person's Google AI plan. What would bill an API key or a cloud project is removed, and it reports no usage to the
//! session.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use nexus_agents::{Agent, AgentT, Antigravity};
use nexus_providers::{Credential, ProviderError};
use serde_json::Value;

use super::LocalAgentT;
use crate::Login;
use crate::launch::{CredentialEnv, LaunchOptions, after, named_model};

/// Antigravity's settings file under the home folder `home`: `modelProvider` there selects its API-key mode.
pub fn antigravity_settings(home: &Path) -> PathBuf {
    crate::shell::profile_under(Agent::Antigravity, home).join("settings.json")
}

/// Whether Antigravity's settings file under `home` makes `agy` bill an API key, read without running `agy`. A
/// `modelProvider` there does ([`Login::ApiKey`]), and with the key gone it refuses to start rather than use the plan.
/// Without one it uses the person's Google sign-in, which its check reads (`/usage`): `None` here. No home folder
/// (`HOME`, else `USERPROFILE`), or a file that is missing or cannot be read as JSON, is `None` too.
pub fn antigravity_login(home: Option<&str>) -> Option<Login> {
    let home = home.filter(|home| !home.is_empty())?;
    let text = std::fs::read_to_string(antigravity_settings(Path::new(home))).ok()?;
    let settings: Value = serde_json::from_str(&text).ok()?;
    settings
        .get("modelProvider")
        .is_some_and(|provider| !provider.is_null())
        .then_some(Login::ApiKey)
}

impl LocalAgentT for Antigravity {
    fn model_catalog<'a>(
        &self,
        _program: &'a std::path::Path,
        _env: &'a HashMap<String, String>,
        _dir: &'a std::path::Path,
    ) -> super::CatalogFuture<'a> {
        Box::pin(async { None })
    }

    fn models<'a>(
        &self,
        program: &'a std::path::Path,
        path: &'a str,
        version: Option<&'a str>,
        env: Option<&'a HashMap<String, String>>,
    ) -> super::ModelFuture<'a> {
        Box::pin(crate::antigravity_models::discover_with_env(
            program, path, version, env,
        ))
    }

    /// [`Login::ApiKey`] when its settings file sets a `modelProvider`.
    fn settled_login(&self, home: Option<&str>) -> Option<Login> {
        antigravity_login(home)
    }

    fn read_status(&self, stdout: &str, _stderr: &str) -> (Login, Option<String>) {
        (crate::read_antigravity_status(stdout), None)
    }

    /// agy's `/usage` names no account.
    fn read_account(&self, _stdout: &str) -> Option<String> {
        None
    }

    /// No Gateway route exists for it, so a caller that skipped `route_supported` still gets the plan's run: the session
    /// it opened with `via: "nexus"` is refused by Gateway, and no call can reach Gateway from here.
    fn launch(&self, options: LaunchOptions) -> (Vec<String>, HashMap<String, String>) {
        let credential = Credential::AgentLogin {
            account_dir: None,
            usage: None,
        };
        self.launch_with_credential(options, credential)
    }

    fn launch_with_credential(
        &self,
        options: LaunchOptions,
        credential: Credential,
    ) -> (Vec<String>, HashMap<String, String>) {
        let mut env = options.env;
        let first = super::route_env(Agent::Antigravity, &credential).apply(&mut env);
        let mut args = after(first, options.args);
        if let Some(model) = options.model.filter(|m| !m.is_empty())
            && named_model(Agent::Antigravity, &args).is_none()
        {
            args.splice(0..0, ["--model".to_string(), model.to_string()]);
        }
        (args, env)
    }

    /// The person's Google sign-in only. Each removed variable makes `agy` bill an API key or a cloud project instead.
    /// The settings file's `modelProvider` does too; `nexus agy` refuses to start while it is set (`settled_login`).
    fn credential_env<'c>(&self, credential: &'c Credential) -> Result<CredentialEnv<'c>, ProviderError> {
        match credential {
            Credential::AgentLogin { .. } => Ok(CredentialEnv {
                set: super::account_dir_env(Agent::Antigravity, credential)
                    .into_iter()
                    .collect(),
                remove: Antigravity.entry().billing.plan_overrides.to_vec(),
                args: Vec::new(),
            }),
            other => Err(ProviderError::NotOffered {
                agent: Agent::Antigravity,
                kind: other.kind(),
            }),
        }
    }

    /// agy's command shells inherit its environment.
    fn shell_variable(&self, _name: &str, _value: &str) -> Option<[String; 2]> {
        None
    }

    fn takes_short_model_flags(&self) -> bool {
        false
    }

    /// `agy` names models by its own ids (`gemini-3.8-flash-high`); no Gateway policy chooses one for it.
    fn provider_model_id(&self, id: &str) -> String {
        id.to_string()
    }

    /// `agy` has no flag that adds to its instructions for one run, and Nexus does not write into its files.
    fn takes_instructions(&self) -> bool {
        false
    }

    fn with_instructions(&self, args: &[String], _text: &str) -> Vec<String> {
        args.to_vec()
    }

    fn takes_recording(&self) -> bool {
        false
    }

    /// Not established for agy: Nexus passes it no prompt to start on.
    fn takes_prompt_argument(&self) -> bool {
        false
    }

    /// The launch refuses `--recording` for it ([`LocalAgentT::takes_recording`]) before this could run.
    fn with_recording(&self, args: &[String], _prompt: &str, _sheet: &Path) -> Vec<String> {
        args.to_vec()
    }

    fn memory(&self, _home: &Path, _repo: &Path) -> std::io::Result<Vec<nexus_agent_memory::NativeEntry>> {
        crate::native_memory::unsupported()
    }

    fn write_memory(&self, _home: &Path, _repo: &Path, _entries: &[nexus_agent_memory::Entry]) -> std::io::Result<()> {
        crate::native_memory::unsupported()
    }

    /// agy keeps each conversation as a SQLite database (`<profile>/conversations/<id>.db`), not a JSON-lines
    /// transcript, so its runs keep no session log.
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
