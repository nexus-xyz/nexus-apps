//! Cursor's local sign-in and subscription launch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use nexus_agents::{Agent, AgentT, Cursor};
use nexus_providers::{Credential, ProviderError};
use serde_json::Value;

use super::{CatalogFuture, LocalAgentT, ModelFuture};
use crate::Login;
use crate::launch::{CredentialEnv, LaunchOptions, after, named_model};

impl LocalAgentT for Cursor {
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

    fn read_status(&self, stdout: &str, _stderr: &str) -> (Login, Option<String>) {
        let status = serde_json::from_str::<Value>(stdout).ok();
        let login = match status.as_ref().and_then(|status| status["status"].as_str()) {
            Some("authenticated") => Login::Plan,
            Some("unauthenticated") => Login::SignedOut,
            _ => Login::Unknown,
        };
        (login, None)
    }

    fn read_account(&self, _stdout: &str) -> Option<String> {
        None
    }

    /// It runs only on its own login, whatever the route: a Gateway route still gets the plan's run.
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
        let first = super::route_env(Agent::Cursor, &credential).apply(&mut env);
        let mut args = after(first, options.args);
        if let Some(model) = options.model.filter(|model| !model.is_empty())
            && named_model(Agent::Cursor, &args).is_none()
        {
            args.splice(0..0, ["--model".to_owned(), model.to_owned()]);
        }
        (args, env)
    }

    /// Its own login only: what would bill another account is removed.
    fn credential_env<'c>(&self, credential: &'c Credential) -> Result<CredentialEnv<'c>, ProviderError> {
        match credential {
            Credential::AgentLogin { .. } => Ok(CredentialEnv {
                set: super::account_dir_env(Agent::Cursor, credential).into_iter().collect(),
                remove: Cursor.entry().billing.plan_overrides.to_vec(),
                args: Vec::new(),
            }),
            other => Err(ProviderError::NotOffered {
                agent: Agent::Cursor,
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
        id.to_string()
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

    /// Not established for cursor-agent: Nexus passes it no prompt to start on.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::Route;

    #[test]
    fn cursor_status_reports_the_plan_or_signed_out_without_account_data() {
        for (stdout, login) in [
            (
                r#"{"status":"authenticated","email":"person@example.invalid"}"#,
                Login::Plan,
            ),
            (r#"{"status":"unauthenticated"}"#, Login::SignedOut),
            (r#"{"status":"other"}"#, Login::Unknown),
            ("", Login::Unknown),
        ] {
            assert_eq!(Cursor.read_status(stdout, ""), (login, None));
        }
    }

    #[test]
    fn cursor_launch_strips_credential_and_endpoint_overrides() {
        let (args, env) = Cursor.launch(LaunchOptions {
            route: Route::Subscription,
            core_url: "https://gateway.invalid",
            api_key: "fake-gateway-key",
            session_id: "fake-session",
            team: None,
            model: None,
            model_catalog: None,
            web_search: false,
            args: vec!["acp".into()],
            env: HashMap::from([
                ("CURSOR_API_KEY".into(), "own".into()),
                ("CURSOR_AUTH_TOKEN".into(), "own".into()),
                ("CURSOR_AUTH_TOKEN_FILE".into(), "own".into()),
                ("CURSOR_API_ENDPOINT".into(), "own".into()),
                ("CURSOR_CONFIG_DIR".into(), "/work/profile".into()),
            ]),
        });
        assert_eq!(args, ["acp"]);
        assert_eq!(
            env,
            HashMap::from([("CURSOR_CONFIG_DIR".into(), "/work/profile".into())])
        );
    }
}
