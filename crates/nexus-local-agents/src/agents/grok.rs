//! Grok uses the person's own login. Model selection for ACP stays in the session protocol.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use nexus_agents::{Agent, Grok};
use nexus_providers::{Credential, ProviderError};

use super::{CatalogFuture, LocalAgentT, ModelFuture};
use crate::Login;
use crate::launch::{CredentialEnv, LaunchOptions};

impl LocalAgentT for Grok {
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

    fn launch(&self, mut options: LaunchOptions) -> (Vec<String>, HashMap<String, String>) {
        crate::grok::subscription_env(&mut options.env);
        (options.args, options.env)
    }

    fn launch_with_credential(
        &self,
        options: LaunchOptions,
        _credential: Credential,
    ) -> (Vec<String>, HashMap<String, String>) {
        self.launch(options)
    }

    fn credential_env<'c>(&self, credential: &'c Credential) -> Result<CredentialEnv<'c>, ProviderError> {
        match credential {
            Credential::AgentLogin { account_dir: None, .. } => Ok(CredentialEnv {
                set: vec![("GROK_DISABLE_API_KEY_AUTH", "true".into())],
                remove: Agent::Grok.entry().billing.plan_overrides.to_vec(),
                args: Vec::new(),
            }),
            other => Err(ProviderError::NotOffered {
                agent: Agent::Grok,
                kind: other.kind(),
            }),
        }
    }

    fn shell_variable(&self, _name: &str, _value: &str) -> Option<[String; 2]> {
        None
    }

    fn takes_short_model_flags(&self) -> bool {
        true
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

    fn takes_prompt_argument(&self) -> bool {
        true
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
